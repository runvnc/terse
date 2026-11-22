//! The device bus seen from the core (DEVICES.md): driver processes
//! started and connected ([`Bus`]), and each machine instance's devices on
//! them ([`BusDevices`], terse-core's `Devices`): the call table's checks,
//! batching, BEGIN and END, state, properties, services and presentation.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

use terse_core::content::{ram_range, readable};
use terse_core::devices::{Backing, CallOutcome, Devices, FrameInputs, GuestMem, Memory};
use terse_core::machines::{ProgramId, StartFailure, failure};
use terse_core::snapshot::{RestoreError, bad};
use terse_core::{DeviceReq, ERR_UNKNOWN, Event, FrameInput, Grants, Image, Machine};

use crate::proto::{self, Addr, CallEntry, DeviceInfo, Hello, LayerDecl, Open, PosEvent, Reader, Writer, access, hello_flags, msg, property};
use crate::stats::Histogram;
use crate::transport::{self, Mapping, ShmEnd, SocketEnd, Spin, UnixListener, UnixStream};

/// How the core reaches one driver.
pub enum Link {
    Socket(SocketEnd),
    Shm(ShmEnd),
}

impl Link {
    fn send(&mut self, packet: &[u8]) -> Result<(), String> {
        match self {
            Link::Socket(s) => s.send(packet),
            Link::Shm(s) => s.send(packet),
        }
    }

    /// The reply (what follows its length) into `buf`.
    fn recv(&mut self, buf: &mut Vec<u8>) -> Result<(), String> {
        match self {
            Link::Socket(s) => s.recv(buf)?.ok_or_else(|| "the driver closed its socket".to_string()),
            Link::Shm(s) => {
                let packet = s.recv()?;
                let n = proto::u32_at(packet.get(..4).ok_or("an empty reply")?, 0) as usize;
                let m = packet.get(4..4 + n).ok_or("a reply cut short")?;
                buf.clear();
                buf.extend_from_slice(m);
                Ok(())
            }
        }
    }

    fn socket(&self) -> &UnixStream {
        match self {
            Link::Socket(s) => &s.stream,
            Link::Shm(s) => &s.socket,
        }
    }
}

/// Per driver measurements.
#[derive(Default)]
pub struct DriverStats {
    /// Round trips of calls that needed a reply.
    pub calls: Histogram,
    /// Calls batched.
    pub batched: u64,
    /// Calls refused by the core's checks without the driver.
    pub checked_out: u64,
    /// Time in BEGIN round trips.
    pub begin: Histogram,
    /// Batches sent.
    pub batches: u64,
    /// Time in PRESENT round trips.
    pub present: Histogram,
    /// GET round trips.
    pub gets: u64,
}

/// What a driver cost the frame loop in the current frame (`PERFORMANCE.md`
/// R14): the time the core waited for its replies, by kind of work.
#[derive(Clone, Debug, Default)]
pub struct Acct {
    /// Work it does every frame (BEGIN, calls, END, PRESENT, GET): the
    /// time it is accountable for.
    pub frame_ns: u64,
    /// Other waits: starting a machine (OPEN, and a machine's first two
    /// frames), host services, state and restore. Recorded, never blamed.
    pub other_ns: u64,
    /// The frame work's time by instance (machine), for naming the program.
    pub by_instance: Vec<(u32, u64)>,
    /// The longest single wait and its kind (`frame`, `start`, `open`,
    /// `service`, `state`).
    pub longest: (u64, &'static str),
}

impl Acct {
    fn add(&mut self, class: &'static str, instance: u32, ns: u64) {
        if class == "frame" {
            self.frame_ns += ns;
            match self.by_instance.iter_mut().find(|(i, _)| *i == instance) {
                Some(e) => e.1 += ns,
                None => self.by_instance.push((instance, ns)),
            }
        } else {
            self.other_ns += ns;
        }
        if ns > self.longest.0 {
            self.longest = (ns, class);
        }
    }

    /// The instance it worked for most in the frame.
    pub fn top_instance(&self) -> Option<u32> {
        self.by_instance.iter().max_by_key(|e| e.1).map(|e| e.0)
    }
}

/// One driver process.
pub struct Proc {
    pub info: DeviceInfo,
    pub transport: u8,
    /// The slots it serves, in increasing order.
    pub slots: Vec<u8>,
    tables: HashMap<u8, Vec<Option<CallEntry>>>,
    link: Link,
    child: Option<Child>,
    /// The batch: CALL messages (framed), and their count.
    batch: Vec<u8>,
    batch_count: u32,
    /// Messages with no reply held for shared memory until the next
    /// request.
    held: Vec<u8>,
    out: Vec<u8>,
    reply: Vec<u8>,
    pub stats: DriverStats,
    /// The command line, for messages.
    pub command: String,
    /// Its process ID (to read its CPU time and memory).
    pub pid: Option<u32>,
    /// This frame's waits for it.
    pub acct: Acct,
    /// An experimental driver's device (DEVICES.md 15): its calls are
    /// charged by the table, it has no inputs and never READs.
    pub experimental: Option<proto::Experimental>,
}

/// How to start the bus.
#[derive(Clone)]
pub struct BusConfig {
    pub session: PathBuf,
    /// Offer shared memory.
    pub shm: bool,
    pub presents: bool,
    pub replay: bool,
    /// Batch calls that allow it.
    pub batching: bool,
    /// Spin of the core's waits for replies on shared memory.
    pub spin: Spin,
    /// Pin each driver to a CPU (in order), if given.
    pub driver_cpus: Vec<usize>,
    /// How long a driver has to connect and answer HELLO.
    pub start_timeout: Duration,
    /// How long a driver has to answer any other request before the core
    /// gives it up as hung (DEVICES.md 10).
    pub reply_timeout: Duration,
}

impl BusConfig {
    pub fn new(session: PathBuf) -> Self {
        BusConfig {
            session,
            shm: true,
            presents: false,
            replay: false,
            batching: true,
            spin: Spin::fixed(Duration::from_micros(100)),
            driver_cpus: Vec::new(),
            start_timeout: Duration::from_secs(10),
            reply_timeout: Duration::from_secs(30),
        }
    }
}

/// The driver processes of a run, shared by every machine of it.
pub struct Bus {
    pub procs: Vec<Proc>,
    by_slot: [Option<usize>; 256],
    pub config: BusConfig,
    /// Why the bus failed, once a driver has (DEVICES.md 10): every
    /// machine using it stops.
    pub failed: Option<String>,
    /// Instances open, for messages.
    pub open: usize,
    /// Instances in their first two frames: their drivers' work for them
    /// is starting a machine, not counted against the drivers (R14).
    pub grace: Vec<u32>,
    /// The host is taking the snapshots the rule asks for (R15).
    pub saving: bool,
    /// Installed drivers not used, and why (DEVICES.md 2.2), for the user.
    pub notes: Vec<Unused>,
}

/// A machine's memory as its memory file (DEVICES.md 4.1): the guest's
/// ROM and RAM, shared with the drivers; the file is removed with it.
pub struct SharedMemory {
    pub map: Mapping,
    pub rom_len: usize,
    pub ram_len: usize,
    pub path: PathBuf,
}

impl SharedMemory {
    /// A new memory file at `path` holding `image`'s ROM and a zeroed RAM.
    pub fn create(path: &Path, image: &Image) -> Result<Self, String> {
        let ram_len = image.ram_pages as usize * 8192;
        let mut map = Mapping::create(path, proto::MEMORY_RAM_OFFSET + ram_len)?;
        map.bytes_mut()[..image.rom.len()].copy_from_slice(&image.rom);
        Ok(SharedMemory { map, rom_len: image.rom.len(), ram_len, path: path.to_path_buf() })
    }
}

impl Backing for SharedMemory {
    fn parts(&self) -> (&[u8], &[u8]) {
        let b = self.map.bytes();
        (&b[..self.rom_len], &b[proto::MEMORY_RAM_OFFSET..][..self.ram_len])
    }

    fn parts_mut(&mut self) -> (&[u8], &mut [u8]) {
        let (rom_len, ram_len) = (self.rom_len, self.ram_len);
        let (rom, rest) = self.map.bytes_mut().split_at_mut(proto::MEMORY_RAM_OFFSET);
        (&rom[..rom_len], &mut rest[..ram_len])
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Splits a command line at whitespace (no quoting: the prototype's
/// commands are paths and options).
pub fn split_command(cmd: &str) -> Vec<String> {
    cmd.split_whitespace().map(str::to_string).collect()
}

/// A driver to start: the slots it is asked for and its command line.
#[derive(Clone, Debug)]
pub struct Wanted {
    pub slots: Vec<u8>,
    pub command: Vec<String>,
    /// An installed driver (DEVICES.md 2.2): what to do if it does not
    /// start or fails its handshake.
    pub installed: Option<Installed>,
    /// An experimental driver (DEVICES.md 15): the name and version of the
    /// device asked for on its one slot. An empty name asks the driver
    /// which device it is (`terse drivers add`); the version is then not
    /// checked.
    pub experimental: Option<(String, u8)>,
}

/// An installed driver that was not used (DEVICES.md 2.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unused {
    pub name: String,
    /// Its group's lowest slot.
    pub slot: u8,
    pub why: String,
}

impl Unused {
    /// The message for standard error.
    pub fn message(&self) -> String {
        if terse_core::image::EXPERIMENTAL.contains(&self.slot) {
            return format!("the installed driver {} for the experimental device on slot {} was not used: {}; the device is not granted", self.name, self.slot, self.why);
        }
        format!("the installed driver {} for slot {} was not used: {}; using the shipped driver (terse drivers use {} default)", self.name, self.slot, self.why, self.slot)
    }
}

/// An installed driver's fallback (DEVICES.md 2.2).
#[derive(Clone, Debug)]
pub struct Installed {
    /// Its name in the drivers folder.
    pub name: String,
    /// The slots and device versions of its manifest: it must answer with
    /// these versions.
    pub versions: Vec<(u8, u8)>,
    /// The shipped driver's command line, if there is one.
    pub shipped: Option<Vec<String>>,
}

impl Bus {
    /// Starts and connects every driver, in order (the order of their
    /// lowest slots).
    pub fn start(config: &BusConfig, drivers: &[Wanted]) -> Result<Bus, String> {
        let mut bus = Bus { procs: Vec::new(), by_slot: [None; 256], config: config.clone(), failed: None, open: 0, grace: Vec::new(), saving: false, notes: Vec::new() };
        let mut drivers = drivers.to_vec();
        drivers.sort_by_key(|w| w.slots.iter().copied().min());
        for (i, want) in drivers.iter().enumerate() {
            let p = match (bus.start_one(i, want), &want.installed) {
                (Ok(p), _) => p,
                (Err(e), None) => return Err(e),
                // DEVICES.md 15.1: an installed experimental driver that
                // cannot be used leaves its device not granted.
                (Err(e), Some(inst)) if want.experimental.is_some() => {
                    bus.notes.push(Unused { name: inst.name.clone(), slot: want.slots[0], why: e });
                    continue;
                }
                // DEVICES.md 2.2: an installed driver that does not start or
                // fails its handshake gives way to the shipped one.
                (Err(e), Some(inst)) => {
                    let g = want.slots.iter().min().copied().unwrap_or(0);
                    let Some(shipped) = &inst.shipped else {
                        return Err(format!("{e} (the installed driver {}; there is no shipped driver for slot {g})", inst.name));
                    };
                    bus.notes.push(Unused { name: inst.name.clone(), slot: g, why: e });
                    bus.start_one(i, &Wanted { slots: want.slots.clone(), command: shipped.clone(), installed: None, experimental: None })?
                }
            };
            for &s in &p.slots {
                if bus.by_slot[s as usize].is_some() {
                    return Err(format!("two drivers for slot {s}"));
                }
                bus.by_slot[s as usize] = Some(bus.procs.len());
            }
            bus.procs.push(p);
        }
        Ok(bus)
    }

    fn start_one(&self, i: usize, want: &Wanted) -> Result<Proc, String> {
        let config = &self.config;
        let name = format!("driver{i}");
        let sock = config.session.join(format!("{name}.sock"));
        // A driver that failed in its place (an installed one, DEVICES.md
        // 2.2) may have left its socket and bus file.
        let _ = std::fs::remove_file(&sock);
        let _ = std::fs::remove_file(config.session.join(format!("{name}.bus")));
        let listener = UnixListener::bind(&sock).map_err(|e| format!("{}: {e}", sock.display()))?;
        let (program, args) = want.command.split_first().ok_or(format!("slots {:?}: an empty command", want.slots))?;
        let what = format!("slots {:?} ({program})", want.slots);
        let mut cmd = Command::new(program);
        cmd.args(args).env("TERSE_BUS_SOCKET", &sock).stdin(Stdio::null());
        if let Some(cpu) = config.driver_cpus.get(i) {
            cmd.env("TERSE_BUS_CPU", cpu.to_string());
        }
        let child = cmd.spawn().map_err(|e| format!("{what}: cannot start: {e}"))?;
        let pid = Some(child.id());
        let mut child = Some(child);
        let fail = |child: &mut Option<Child>, e: String| {
            reap(child);
            format!("{what}: {e}")
        };
        let stream = match accept(&listener, config.start_timeout, child.as_mut().unwrap()) {
            Ok(s) => s,
            Err(e) => return Err(fail(&mut child, e)),
        };
        let _ = std::fs::remove_file(&sock);
        let bus_path = config.session.join(format!("{name}.bus"));
        if config.shm {
            let m = Mapping::create(&bus_path, transport::BUS_FILE)?;
            transport::init_bus(&m);
        }
        let mut flags = 0;
        if config.shm {
            flags |= hello_flags::SHM;
        }
        if config.presents {
            flags |= hello_flags::PRESENTS;
        }
        if config.replay {
            flags |= hello_flags::REPLAY;
        }
        let hello = Hello {
            flags,
            slots: want.slots.clone(),
            session_dir: config.session.display().to_string(),
            bus_path: if config.shm { bus_path.display().to_string() } else { String::new() },
            experimental: want.experimental.iter().map(|(name, _)| (want.slots[0], name.clone())).collect(),
        };
        let mut out = Vec::new();
        hello.encode(&mut out);
        let mut sock_end = SocketEnd::new(stream)?;
        sock_end.set_deadline(Some(config.start_timeout));
        let mut reply = Vec::new();
        let handshake = sock_end.send(&out).and_then(|_| sock_end.recv(&mut reply)?.ok_or_else(|| "closed before its HELLO reply".to_string()));
        if let Err(e) = handshake {
            return Err(fail(&mut child, e));
        }
        let decoded = proto::split(&reply).map_err(String::from).and_then(|m| DeviceInfo::decode(m.body));
        let (transport, mut info) = match decoded {
            Ok(v) => v,
            Err(e) => return Err(fail(&mut child, e)),
        };
        // DEVICES.md 15.1, 15.2: an experimental driver's declaration.
        let experimental = match &want.experimental {
            None => {
                info.experimental.clear();
                None
            }
            Some((name, version)) => {
                let slot = want.slots[0];
                let checked = info.check_experimental(slot, name).and_then(|_| match info.slots[0].version {
                    v if name.is_empty() || v == *version => Ok(()),
                    v => Err(format!("it serves {name} version {v}, not {version}")),
                });
                if let Err(e) = checked {
                    return Err(fail(&mut child, e));
                }
                info.experimental.first().cloned()
            }
        };
        let mut slots: Vec<u8> = info.slots.iter().map(|s| s.slot).collect();
        slots.sort_unstable();
        let mut asked = want.slots.clone();
        asked.sort_unstable();
        // Full text (10, a capability) is asked of the terminal's driver; one
        // that does not answer for it does not give it (DEVICES.md 1.1).
        let full_text = terse_core::text::FULL_TEXT;
        let without_full_text: Vec<u8> = asked.iter().copied().filter(|&s| s != full_text).collect();
        if (slots != asked && slots != without_full_text) ||(transport == proto::TRANSPORT_SHM && !config.shm) || transport > 1 {
            return Err(fail(&mut child, format!("it answered for slots {slots:?}, transport {transport}")));
        }
        // An installed driver answers with the versions it was installed
        // with (DEVICES.md 2.2).
        if let Some(inst) = &want.installed
            && experimental.is_none()
            && let Some(s) = info.slots.iter().find(|s| !inst.versions.contains(&(s.slot, s.version)))
        {
            let was = inst.versions.iter().find(|v| v.0 == s.slot).map_or("not installed for it".to_string(), |v| format!("installed as version {}", v.1));
            return Err(fail(&mut child, format!("it answered for device {} version {}, {was}", s.slot, s.version)));
        }
        let link = if transport == proto::TRANSPORT_SHM {
            let m = Mapping::open(&bus_path, true)?;
            let mut end = ShmEnd::new(m, true, config.spin, sock_end.stream)?;
            end.deadline = Some(config.reply_timeout);
            Link::Shm(end)
        } else {
            sock_end.set_deadline(Some(config.reply_timeout));
            Link::Socket(sock_end)
        };
        let mut tables = HashMap::new();
        for s in &info.slots {
            let mut table = vec![None; 256];
            for e in &s.table {
                table[e.function as usize] = Some(*e);
            }
            tables.insert(s.slot, table);
        }
        Ok(Proc {
            info,
            transport,
            slots,
            tables,
            link,
            child,
            batch: Vec::new(),
            batch_count: 0,
            held: Vec::new(),
            out: Vec::new(),
            reply: Vec::new(),
            stats: DriverStats::default(),
            command: want.command.join(" "),
            pid,
            acct: Acct::default(),
            experimental,
        })
    }

    /// The driver serving `slot`.
    pub fn proc_for(&self, slot: u8) -> Option<usize> {
        self.by_slot[slot as usize]
    }

    /// The first native size a driver proposed.
    pub fn native(&self) -> Option<(u32, u32)> {
        self.procs.iter().map(|d| d.info.native).find(|&(w, h)| w > 0 && h > 0).map(|(w, h)| (w as u32, h as u32))
    }

    /// Records a driver's failure: the run ends (DEVICES.md 10).
    fn fail(&mut self, p: usize, e: String) -> String {
        let d = &mut self.procs[p];
        let status = d.child.as_mut().and_then(|c| c.try_wait().ok().flatten());
        let what = match status {
            Some(s) => format!("driver {} ({}) exited ({s}): {e}", d.info.name, d.command),
            None => format!("driver {} ({}): {e}", d.info.name, d.command),
        };
        if self.failed.is_none() {
            self.failed = Some(what.clone());
        }
        what
    }

    /// Ends the session: SHUTDOWN to each driver, then waits for it (and
    /// kills one that does not exit).
    pub fn shutdown(&mut self) {
        for d in &mut self.procs {
            d.out.clear();
            d.out.append(&mut d.held);
            d.take_batch();
            Writer::new(&mut d.out, Addr::PROCESS, msg::SHUTDOWN).finish();
            let out = std::mem::take(&mut d.out);
            let _ = d.link.send(&out);
            let _ = d.link.socket().shutdown(std::net::Shutdown::Both);
        }
        for d in &mut self.procs {
            reap(&mut d.child);
        }
    }

    /// Kills every driver at once (a signal, a failure).
    pub fn kill(&mut self) {
        for d in &mut self.procs {
            if let Some(c) = &mut d.child {
                let _ = c.kill();
                let _ = c.wait();
            }
            d.child = None;
        }
    }

    /// A request to driver `p` (body written by `f` after the type byte),
    /// with what is held and batched in front; waits for its reply,
    /// serving READs from `mem`. Returns the reply's body.
    fn request(&mut self, p: usize, to: Addr, kind: u8, f: impl FnOnce(&mut Writer), mem: ReadMem) -> Result<Vec<u8>, String> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        let start = Instant::now();
        let r = self.procs[p].request(to, kind, f, mem);
        let class = self.class(kind, to.instance);
        self.procs[p].acct.add(class, to.instance, start.elapsed().as_nanos() as u64);
        r.map_err(|e| self.fail(p, e))
    }

    /// What kind of work a request is, for the accounts (R14).
    fn class(&self, kind: u8, instance: u32) -> &'static str {
        match kind {
            msg::OPEN => "open",
            msg::SERVICE => "service",
            // A device's state is its own work (hashing, a snapshot the
            // shell asked for, and the first request after END waits for
            // END's work too), but not the snapshot the rule itself takes.
            msg::STATE | msg::RESTORE | msg::RESET if self.saving => "state",
            _ if self.grace.contains(&instance) => "start",
            _ => "frame",
        }
    }

    /// Sets how long every driver has to answer (DEVICES.md 10); the
    /// host shortens it while it takes the snapshot of a machine whose
    /// device is slow (R15).
    pub fn set_reply_timeout(&mut self, d: Duration) {
        for p in &mut self.procs {
            match &mut p.link {
                Link::Socket(s) => s.set_deadline(Some(d)),
                Link::Shm(s) => s.deadline = Some(d),
            }
        }
    }

    /// Sends what is batched for every driver (on the socket; on shared
    /// memory it goes with the next request).
    pub fn flush(&mut self) -> Result<(), String> {
        for p in 0..self.procs.len() {
            let d = &mut self.procs[p];
            if (d.batch_count > 0 || !d.held.is_empty()) && matches!(d.link, Link::Socket(_)) {
                d.out.clear();
                d.out.append(&mut d.held);
                d.take_batch();
                let out = std::mem::take(&mut d.out);
                let r = d.link.send(&out);
                d.out = out;
                r.map_err(|e| self.fail(p, e))?;
            }
        }
        Ok(())
    }

    /// A message with no reply: at once on the socket, held for the next
    /// request on shared memory (DEVICES.md 4.2).
    fn post(&mut self, p: usize, to: Addr, kind: u8, f: impl FnOnce(&mut Writer)) -> Result<(), String> {
        let d = &mut self.procs[p];
        let mut m = Vec::new();
        let mut w = Writer::new(&mut m, to, kind);
        f(&mut w);
        w.finish();
        match d.link {
            // In order after what is held and batched.
            Link::Shm(_) => {
                d.out.clear();
                d.out.append(&mut d.held);
                d.take_batch();
                d.out.extend_from_slice(&m);
                std::mem::swap(&mut d.out, &mut d.held);
                Ok(())
            }
            Link::Socket(_) => {
                d.out.clear();
                d.out.append(&mut d.held);
                d.take_batch();
                d.out.extend_from_slice(&m);
                let out = std::mem::take(&mut d.out);
                let r = d.link.send(&out);
                d.out = out;
                r.map_err(|e| self.fail(p, e))
            }
        }
    }

    /// A host service (DEVICES.md 9) of whichever driver gives it, for
    /// `instance`.
    pub fn service(&mut self, instance: u32, name: &str, request: &[u8]) -> Result<Vec<u8>, String> {
        let p = self.procs.iter().position(|d| d.info.services.iter().any(|s| s == name)).ok_or(format!("no driver gives the service {name}"))?;
        let reply = self.request(p, Addr::of(instance), msg::SERVICE, |w| {
            w.u8(0).str(name.as_bytes()).blob(request);
        }, None)?;
        let mut r = Reader::new(&reply);
        r.skip(1)?;
        let ok = r.u8()?;
        let message = r.string()?;
        let data = r.blob()?.to_vec();
        if ok == 1 { Ok(data) } else { Err(message) }
    }

    /// Whether some driver gives `name`.
    pub fn has_service(&self, name: &str) -> bool {
        self.procs.iter().any(|d| d.info.services.iter().any(|s| s == name))
    }

    /// Whether any driver has exited, or its socket has closed (between
    /// frames: a crash noticed before the next request).
    pub fn check_alive(&mut self) -> Result<(), String> {
        for p in 0..self.procs.len() {
            let gone = transport::peer_gone(self.procs[p].link.socket())
                || self.procs[p].child.as_mut().is_some_and(|c| c.try_wait().ok().flatten().is_some());
            if gone {
                return Err(self.fail(p, "it has gone".into()));
            }
        }
        Ok(())
    }

    /// Whether a driver gives device `d` (its slot, in its version).
    pub fn provides(&self, d: &DeviceReq) -> bool {
        self.by_slot[d.slot as usize].is_some_and(|p| self.procs[p].experimental.is_none() && self.procs[p].info.slots.iter().any(|s| s.slot == d.slot && s.version == d.version))
    }

    /// Whether a driver gives the experimental device `d` named `name`, in
    /// its version (DEVICES.md 15.1).
    pub fn provides_experimental(&self, d: &DeviceReq, name: &str) -> bool {
        self.by_slot[d.slot as usize].is_some_and(|p| {
            let x = &self.procs[p];
            x.experimental.as_ref().is_some_and(|e| e.slot == d.slot && e.name == name) && x.info.slots.iter().any(|s| s.slot == d.slot && s.version == d.version)
        })
    }

    /// Opens a machine instance on the drivers (OPEN): `image` with its
    /// memory file (`memory`) and identity. The image's required devices
    /// other than System and Machines must all have drivers (SPEC §image), and
    /// so must the optional ones `grants` grants; `None`: every optional
    /// device a driver gives is granted.
    pub fn open_instance(
        this: &Rc<RefCell<Bus>>,
        instance: u32,
        image: &Image,
        grants: Option<&Grants>,
        identity: Option<&ProgramId>,
    ) -> Result<Machine<BusDevices>, StartFailure> {
        let mut bus = this.borrow_mut();
        let grants = match grants {
            Some(g) => {
                image.check_grants_on(g, true).map_err(|d| terse_core::machines::needs_device(image, d))?;
                g.clone()
            }
            None => image.grants_on(|d| bus.provides(d), |d, name| bus.provides_experimental(d, name)),
        };
        let has = image.device_set(&grants);
        let mut slot_proc = [None; 256];
        let mut procs: Vec<usize> = Vec::new();
        for d in &image.devices {
            if d.slot == 1 || d.slot == terse_core::machines::SLOT || !has.has(d.slot) {
                continue;
            }
            let Some(p) = bus.by_slot[d.slot as usize] else {
                return Err(StartFailure { reason: failure::NEEDS_DEVICE, image: Some(image.id), message: format!("no driver for device {}", d.slot) });
            };
            // An experimental slot goes only to the driver for its name.
            let named = image.experimental_name(d.slot);
            if d.experimental() != bus.procs[p].experimental.is_some() || (d.experimental() && !named.as_deref().is_some_and(|n| bus.provides_experimental(d, n))) {
                return Err(StartFailure {
                    reason: failure::NEEDS_DEVICE,
                    image: Some(image.id),
                    message: format!("no driver for the experimental device {} on slot {}", named.unwrap_or_default(), d.slot),
                });
            }
            let version = bus.procs[p].info.slots.iter().find(|s| s.slot == d.slot).map(|s| s.version);
            if version != Some(d.version) {
                return Err(StartFailure {
                    reason: failure::NEEDS_DEVICE,
                    image: Some(image.id),
                    message: format!("the driver for device {} has version {version:?}, not {}", d.slot, d.version),
                });
            }
            slot_proc[d.slot as usize] = Some(p);
            if !procs.contains(&p) {
                procs.push(p);
            }
        }
        // Device 9's style lives with device 3 (SPEC §vector3d.calls).
        if has.has(terse_core::vector3::SLOT) && slot_proc[3].is_none() {
            let p = slot_proc[terse_core::vector3::SLOT as usize];
            if bus.by_slot[3] != p {
                return Err(StartFailure { reason: failure::NEEDS_DEVICE, image: Some(image.id), message: "devices 3 and 9 need one driver".into() });
            }
            slot_proc[3] = p;
        }
        procs.sort_unstable();
        let path = bus.config.session.join(format!("memory-{instance}"));
        let memory = SharedMemory::create(&path, image).map_err(|e| StartFailure { reason: failure::INVALID, image: Some(image.id), message: e })?;
        for &p in &procs {
            let shm = bus.procs[p].transport == proto::TRANSPORT_SHM;
            let open = Open {
                image_id: image.id,
                rom_len: image.rom.len() as u32,
                ram_len: memory.ram_len as u32,
                memory_path: if shm { path.display().to_string() } else { String::new() },
                // Only the devices the machine has (SPEC §image).
                listed: image.devices.iter().filter(|d| has.has(d.slot) && bus.procs[p].slots.contains(&d.slot)).map(|d| (d.slot, d.version)).collect(),
                identity: identity.map(|i| (i.publisher, i.name.clone())),
                rom: image.rom.clone(),
            };
            let mut m = Vec::new();
            open.encode(instance, &mut m);
            let to = Addr::of(instance);
            let reply = bus.request(p, to, msg::OPEN, |w| {
                // The message is already written: take its body.
                w.bytes(&proto::split(&m[4..]).unwrap().body[1..]);
            }, None);
            let fail = |e: String| StartFailure { reason: failure::INVALID, image: Some(image.id), message: e };
            let reply = reply.map_err(fail)?;
            let mut r = Reader::new(&reply);
            r.skip(1).map_err(|e| fail(e.into()))?;
            if r.u8().map_err(|e| fail(e.into()))? != 1 {
                r.skip(2).map_err(|e| fail(e.into()))?;
                let why = r.string().unwrap_or_default();
                return Err(StartFailure { reason: failure::NEEDS_DEVICE, image: Some(image.id), message: format!("{}: {why}", bus.procs[p].info.name) });
            }
        }
        bus.open += 1;
        let layers = procs
            .iter()
            .flat_map(|&p| {
                let d = &bus.procs[p];
                d.info.slots.iter().filter(move |s| slot_proc[s.slot as usize] == Some(p)).flat_map(move |s| s.layers.iter().enumerate().map(move |(i, l)| (l.z, s.slot, i, *l)))
            })
            .collect::<Vec<_>>();
        let mut layers = layers;
        layers.sort_by_key(|&(z, s, i, _)| (z, s, i));
        drop(bus);
        let devices = BusDevices {
            bus: this.clone(),
            instance,
            procs,
            slot_proc,
            layers: layers.into_iter().map(|(_, s, _, l)| (s, l)).collect(),
            list: RefCell::new(None),
            vector: RefCell::new(None),
            dirty: Cell::new(u64::MAX),
            got: RefCell::new(HashMap::new()),
            sent: HashMap::new(),
            replay: None,
            records: Vec::new(),
            audio_frame: None,
            frames_begun: 0,
        };
        Ok(Machine::with_devices(image, grants, Memory::Backed(Box::new(memory)), devices))
    }
}

/// Guest memory a READ may be served from: ROM and RAM.
type ReadMem<'a> = Option<(&'a [u8], &'a [u8])>;

/// A layer's news from PRESENT.
pub struct LayerUpdate {
    pub slot: u8,
    pub id: u8,
    pub path: Option<PathBuf>,
    pub damage: Vec<[i32; 4]>,
}

/// Accepts the driver's connection, giving up after `timeout` or if it
/// exits first.
fn accept(listener: &UnixListener, timeout: Duration, child: &mut Child) -> Result<UnixStream, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        match listener.accept() {
            Ok((s, _)) => {
                s.set_nonblocking(false).map_err(|e| e.to_string())?;
                return Ok(s);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.to_string()),
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("exited ({status}) before it connected"));
        }
        if start.elapsed() > timeout {
            return Err(format!("did not connect within {:.1} s", timeout.as_secs_f64()));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Waits up to 2 s for a driver to exit, then kills it.
fn reap(child: &mut Option<Child>) {
    let Some(mut c) = child.take() else { return };
    let start = Instant::now();
    loop {
        match c.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if start.elapsed() < Duration::from_secs(2) => std::thread::sleep(Duration::from_millis(2)),
            _ => {
                let _ = c.kill();
                let _ = c.wait();
                return;
            }
        }
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Proc {
    /// Puts the batch (if any) after `out`'s contents.
    fn take_batch(&mut self) {
        if self.batch_count == 0 {
            return;
        }
        let mut w = Writer::new(&mut self.out, Addr::PROCESS, msg::BATCH);
        w.u8(0).u16(0).u32(self.batch_count);
        // Each call: u32 length, then the message (address and body).
        let mut at = 0;
        while at < self.batch.len() {
            let n = proto::u32_at(&self.batch, at) as usize;
            w.blob(&self.batch[at + 4..at + 4 + n]);
            at += 4 + n;
        }
        w.finish();
        self.stats.batches += 1;
        self.batch.clear();
        self.batch_count = 0;
    }

    /// Serves a READ from `mem`.
    fn data(&mut self, to: Addr, reply: &[u8], mem: ReadMem) -> Result<(), String> {
        let mut r = Reader::new(&reply[1..]);
        r.skip(3)?;
        let (addr, len) = (r.u32()?, r.u32()? as usize);
        self.out.clear();
        let mut w = Writer::new(&mut self.out, to, msg::DATA);
        match mem.and_then(|(rom, ram)| readable(rom, ram, addr, len)) {
            Some(b) => w.u8(1).u16(0).bytes(b),
            None => w.u8(0).u16(0),
        };
        w.finish();
        let out = std::mem::take(&mut self.out);
        let r = self.link.send(&out);
        self.out = out;
        r
    }

    /// Waits for the reply of type `kind | REPLY` for `to`, serving READs.
    fn wait_reply(&mut self, to: Addr, kind: u8, mem: ReadMem) -> Result<Vec<u8>, String> {
        let mut reply = std::mem::take(&mut self.reply);
        loop {
            self.link.recv(&mut reply)?;
            let m = proto::split(&reply)?;
            let body = m.body.to_vec();
            match body.first() {
                Some(&t) if t == kind | msg::REPLY => {
                    if m.to.instance != to.instance {
                        return Err(format!("a reply for instance {} where {} was asked", m.to.instance, to.instance));
                    }
                    self.reply = reply;
                    return Ok(body);
                }
                // DEVICES.md 15.3: an experimental driver never READs.
                Some(&msg::READ) if self.experimental.is_some() => return Err("an experimental driver sent READ (DEVICES.md 15.3)".into()),
                Some(&msg::READ) => self.data(to, &body, mem)?,
                Some(&msg::ERROR) => return Err(String::from_utf8_lossy(&body[1..]).into_owned()),
                other => return Err(format!("unexpected message {other:02x?} for {kind:02x}")),
            }
        }
    }

    fn request(&mut self, to: Addr, kind: u8, f: impl FnOnce(&mut Writer), mem: ReadMem) -> Result<Vec<u8>, String> {
        self.send_request(to, kind, f)?;
        self.wait_reply(to, kind, mem)
    }

    /// Sends a request (with what is held and batched in front) without
    /// waiting for its reply.
    fn send_request(&mut self, to: Addr, kind: u8, f: impl FnOnce(&mut Writer)) -> Result<(), String> {
        self.out.clear();
        self.out.append(&mut self.held);
        self.take_batch();
        let mut w = Writer::new(&mut self.out, to, kind);
        f(&mut w);
        w.finish();
        let out = std::mem::take(&mut self.out);
        let sent = self.link.send(&out);
        self.out = out;
        sent
    }
}

/// Copies writes (address, length, bytes) into RAM, each within `[lo, hi)`.
fn apply_writes(r: &mut Reader, ram: &mut [u8], within: Option<(u32, u32)>) -> Result<(), String> {
    let w = r.u32()?;
    for _ in 0..w {
        let addr = r.u32()?;
        let bytes = r.blob()?;
        let ok = match within {
            Some((lo, hi)) => addr >= lo && (addr as u64 + bytes.len() as u64) <= hi as u64,
            None => true,
        };
        let at = ram_range(ram.len(), addr, bytes.len()).filter(|_| ok).ok_or("a write outside its buffer")?;
        ram[at..at + bytes.len()].copy_from_slice(bytes);
    }
    Ok(())
}

/// One machine instance's devices on the bus (terse-core's `Devices`).
pub struct BusDevices {
    pub bus: Rc<RefCell<Bus>>,
    pub instance: u32,
    /// The drivers serving it, in order.
    procs: Vec<usize>,
    /// The driver of each slot it lists (3 also for an image with 9 only).
    slot_proc: [Option<usize>; 256],
    /// Its layers bottom to top: (slot, layer).
    pub layers: Vec<(u8, LayerDecl)>,
    /// Device 9's list as last read (`vector3.list`).
    list: RefCell<Option<Vec<u8>>>,
    /// Device 3's state as last read (`vector.list`), kept as one `Vector`
    /// whose generation moves when its list does.
    vector: RefCell<Option<terse_core::Vector>>,
    /// Drivers (bit per index) that may have changed what they publish
    /// since the core last asked.
    dirty: Cell<u64>,
    /// Published values last read, by driver and name.
    got: RefCell<HashMap<(usize, String), Vec<u8>>>,
    /// Values last given to each subscribing driver.
    sent: HashMap<(usize, String), Vec<u8>>,
    /// The drivers' recorded inputs for the next frame (replay), by the
    /// driver's lowest slot.
    pub replay: Option<Vec<(u8, Vec<u8>)>>,
    /// The drivers' input records of the last frame (for recording).
    pub records: Vec<(u8, Vec<u8>)>,
    /// The frame of the sound driver's samples taken last.
    audio_frame: Option<u32>,
    /// Frames begun since it was opened.
    frames_begun: u32,
}

impl BusDevices {
    fn touch(&self, p: usize) {
        self.dirty.set(self.dirty.get() | 1 << p);
        if self.slot_proc[terse_core::vector3::SLOT as usize] == Some(p) {
            *self.list.borrow_mut() = None;
        }
    }

    /// The drivers serving this instance.
    pub fn procs(&self) -> &[usize] {
        &self.procs
    }

    /// Device 3's state as the vector driver publishes it (`vector.list`):
    /// the style, persistence, fill mode and 2D list, for the system ink
    /// and the phosphor style's beam (DEVICES.md 7.1). `None` if no driver
    /// of this machine publishes it.
    pub fn vector(&self) -> Option<terse_core::Vector> {
        let bytes = self.property(property::LIST2).ok().flatten()?;
        let mut cache = self.vector.borrow_mut();
        let v = cache.get_or_insert_with(terse_core::Vector::default);
        let [style, persistence, outlines] = *bytes.first_chunk::<3>()?;
        let n = u32::from_le_bytes(*bytes.get(3..)?.first_chunk::<4>()?) as usize;
        let list = bytes.get(7..7 + n)?;
        v.set_style(style as u32);
        v.set_persistence(persistence as u32);
        v.set_fill(outlines as u32);
        if list.is_empty() {
            v.clear();
        } else {
            v.submit(list);
        }
        Some(v.clone())
    }

    /// The slots of this machine's layers whose drivers draw light when
    /// told to (they subscribe to `core.light`), but the vector displays'
    /// (DEVICES.md 7.1).
    pub fn light_slots(&self) -> Vec<u8> {
        let bus = self.bus.borrow();
        let mut out: Vec<u8> = Vec::new();
        for (s, _) in &self.layers {
            let Some(p) = self.slot_proc[*s as usize] else { continue };
            if *s != 3 && *s != terse_core::vector3::SLOT && bus.procs[p].info.subscribes.iter().any(|n| n == property::LIGHT) && !out.contains(s) {
                out.push(*s);
            }
        }
        out
    }

    /// A property published by a driver of this instance, read again if the
    /// driver may have changed it (DEVICES.md 7.2).
    pub fn property(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let mut bus = self.bus.borrow_mut();
        let Some(&p) = self.procs.iter().find(|&&p| bus.procs[p].info.publishes.iter().any(|n| n == name)) else { return Ok(None) };
        let key = (p, name.to_string());
        if self.dirty.get() & 1 << p == 0
            && let Some(v) = self.got.borrow().get(&key)
        {
            return Ok(Some(v.clone()));
        }
        // Everything this driver publishes, at once.
        let names = bus.procs[p].info.publishes.clone();
        let reply = bus.request(p, Addr::of(self.instance), msg::GET, |w| {
            w.u8(0).u8(names.len() as u8);
            for n in &names {
                w.str(n.as_bytes());
            }
        }, None)?;
        bus.procs[p].stats.gets += 1;
        let mut r = Reader::new(&reply);
        r.skip(1)?;
        let n = r.u8()?;
        let mut got = self.got.borrow_mut();
        for _ in 0..n {
            let k = r.string()?;
            got.insert((p, k), r.blob()?.to_vec());
        }
        self.dirty.set(self.dirty.get() & !(1 << p));
        Ok(got.get(&key).cloned())
    }

    /// Gives every driver of this instance that subscribes to `name` its
    /// new value (SET), if it changed.
    pub fn set_property(&mut self, name: &str, value: &[u8]) -> Result<(), String> {
        let mut bus = self.bus.borrow_mut();
        for &p in &self.procs {
            if !bus.procs[p].info.subscribes.iter().any(|n| n == name) {
                continue;
            }
            let key = (p, name.to_string());
            if self.sent.get(&key).is_some_and(|v| v == value) {
                continue;
            }
            bus.post(p, Addr::of(self.instance), msg::SET, |w| {
                w.u8(0).str(name.as_bytes()).blob(value);
            })?;
            self.sent.insert(key, value.to_vec());
            self.dirty.set(self.dirty.get() | 1 << p);
        }
        Ok(())
    }

    /// The properties drivers of this instance publish and others
    /// subscribe to, carried over (before PRESENT).
    pub fn carry_properties(&mut self) -> Result<(), String> {
        let wanted: Vec<String> = {
            let bus = self.bus.borrow();
            let mut v: Vec<String> = Vec::new();
            for &p in &self.procs {
                for n in &bus.procs[p].info.subscribes {
                    let published = self.procs.iter().any(|&q| q != p && bus.procs[q].info.publishes.contains(n));
                    if published && !v.contains(n) {
                        v.push(n.clone());
                    }
                }
            }
            v
        };
        for name in wanted {
            if let Some(value) = self.property(&name)? {
                self.set_property(&name, &value)?;
            }
        }
        Ok(())
    }

    /// PRESENT to each driver of this instance with layers (DEVICES.md 7):
    /// a picture of `picture` pixels showing the screen in `screen`, glyph
    /// cells `cell` pixels wide (0: native). Returns each layer's news.
    pub fn present(&mut self, picture: (u32, u32), screen: (f32, f32, f32, f32), cell: u32) -> Result<Vec<LayerUpdate>, String> {
        self.carry_properties()?;
        let mut bus = self.bus.borrow_mut();
        bus.flush()?;
        let mut updates = Vec::new();
        if let Some(e) = &bus.failed {
            return Err(e.clone());
        }
        // PRESENT to every driver with layers first, then collect: the
        // drivers draw at once (as BEGIN). Each is accountable for the time
        // the core waited for it after the ones before (R14).
        let to = Addr::of(self.instance);
        let mut sent = Vec::new();
        for &p in &self.procs {
            if !self.layers.iter().any(|(s, _)| self.slot_proc[*s as usize] == Some(p)) {
                continue;
            }
            let start = Instant::now();
            bus.procs[p]
                .send_request(to, msg::PRESENT, |w| {
                    w.u8(0).u16(0).u16(picture.0 as u16).u16(picture.1 as u16);
                    w.f32(screen.0).f32(screen.1).f32(screen.2).f32(screen.3).u16(cell as u16).u16(0);
                })
                .map_err(|e| bus.fail(p, e))?;
            sent.push((p, start));
        }
        for (p, start) in sent {
            let waited = Instant::now();
            let reply = bus.procs[p].wait_reply(to, msg::PRESENT, None).map_err(|e| bus.fail(p, e))?;
            let class = bus.class(msg::PRESENT, self.instance);
            let d = &mut bus.procs[p];
            d.acct.add(class, self.instance, waited.elapsed().as_nanos() as u64);
            d.stats.present.add(start.elapsed());
            let mut r = Reader::new(&reply);
            r.u8()?;
            let k = r.u8()?;
            r.skip(2)?;
            for _ in 0..k {
                let (slot, id, flags) = (r.u8()?, r.u8()?, r.u8()?);
                let path = if flags & 1 != 0 { Some(PathBuf::from(r.string()?)) } else { None };
                let n = r.u32()?;
                let mut damage = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    damage.push([r.i32()?, r.i32()?, r.i32()?, r.i32()?]);
                }
                updates.push(LayerUpdate { slot, id, path, damage });
            }
        }
        Ok(updates)
    }

    /// The slot's state section (STATE).
    pub fn state(&mut self, slot: u8) -> Result<Vec<u8>, String> {
        let p = self.slot_proc[slot as usize].ok_or(format!("no driver for slot {slot}"))?;
        let mut bus = self.bus.borrow_mut();
        let reply = bus.request(p, Addr::new(self.instance, slot), msg::STATE, |_| {}, None)?;
        let mut r = Reader::new(&reply);
        r.skip(4)?;
        let section = r.blob()?.to_vec();
        // DEVICES.md 15.3: at most what the experimental driver declared.
        if let Some(x) = &bus.procs[p].experimental
            && section.len() > x.state_max as usize
        {
            let e = format!("a state section of {} bytes, more than the {} it declared (DEVICES.md 15.3)", section.len(), x.state_max);
            return Err(bus.fail(p, e));
        }
        Ok(section)
    }

    /// Restores the slot's section from the start of `bytes` (RESTORE):
    /// its length, or `None` if refused.
    pub fn restore(&mut self, slot: u8, bytes: &[u8]) -> Result<Option<usize>, String> {
        let p = self.slot_proc[slot as usize].ok_or("no such driver")?;
        self.touch(p);
        let reply = self.bus.borrow_mut().request(p, Addr::new(self.instance, slot), msg::RESTORE, |w| {
            w.u8(0).u16(0).blob(bytes);
        }, None)?;
        let mut r = Reader::new(&reply);
        r.skip(1)?;
        let ok = r.u8()? == 1;
        r.skip(2)?;
        let n = r.u32()? as usize;
        Ok(ok.then_some(n))
    }

    /// Asks every driver of this instance to reset (RESET).
    pub fn reset(&mut self) -> Result<(), String> {
        for &p in &self.procs {
            self.touch(p);
            self.bus.borrow_mut().request(p, Addr::of(self.instance), msg::RESET, |_| {}, None)?;
        }
        Ok(())
    }

    /// The driver records of the frame just run.
    pub fn take_records(&mut self) -> Vec<(u8, Vec<u8>)> {
        std::mem::take(&mut self.records)
    }
}

impl Drop for BusDevices {
    fn drop(&mut self) {
        let Ok(mut bus) = self.bus.try_borrow_mut() else { return };
        for &p in &self.procs {
            let _ = bus.post(p, Addr::of(self.instance), msg::CLOSE, |_| {});
        }
        bus.open = bus.open.saturating_sub(1);
        bus.grace.retain(|&i| i != self.instance);
    }
}

impl Devices for BusDevices {
    fn begin_frame(&mut self, frame: u32, input: &FrameInput, mem: GuestMem) -> Result<FrameInputs, String> {
        let (time, host) = (input.unix_time, input.events);
        if !input.completions.is_empty() {
            return Err("the bus's content driver gives the completions".into());
        }
        let replay = self.replay.take();
        self.records.clear();
        // A machine's first two frames start it (R14).
        {
            let mut bus = self.bus.borrow_mut();
            bus.grace.retain(|&i| i != self.instance);
            if self.frames_begun < 2 {
                bus.grace.push(self.instance);
            }
        }
        self.frames_begun += 1;
        let to = Addr::of(self.instance);
        let mut inputs = FrameInputs::default();
        let mut bus = self.bus.borrow_mut();
        if let Some(e) = &bus.failed {
            return Err(e.clone());
        }
        // Send BEGIN to all, then collect: the drivers work at once.
        let mut sent = Vec::new();
        for &p in &self.procs {
            self.dirty.set(self.dirty.get() | 1 << p);
            let d = &mut bus.procs[p];
            let first = d.slots[0];
            d.out.clear();
            d.out.append(&mut d.held);
            d.take_batch();
            let mut w = Writer::new(&mut d.out, to, msg::BEGIN);
            let record = replay.as_ref().map(|r| r.iter().find(|(s, _)| *s == first).map_or(&[][..], |(_, b)| &b[..]));
            w.u8(record.is_some() as u8).u16(0).u32(frame).u32(time).u32(host.len() as u32);
            for e in host {
                w.bytes(&[e.kind, e.a, e.b, e.c]);
            }
            w.blob(record.unwrap_or(&[]));
            w.finish();
            let out = std::mem::take(&mut d.out);
            let r = d.link.send(&out);
            d.out = out;
            r.map_err(|e| bus.fail(p, e))?;
            sent.push((p, Instant::now()));
        }
        *self.list.borrow_mut() = None;
        for (p, start) in sent {
            let waited = Instant::now();
            let reply = bus.procs[p].wait_reply(to, msg::BEGIN, Some((mem.rom, &*mem.ram))).map_err(|e| bus.fail(p, e))?;
            let class = bus.class(msg::BEGIN, self.instance);
            let d = &mut bus.procs[p];
            d.acct.add(class, self.instance, waited.elapsed().as_nanos() as u64);
            d.stats.begin.add(start.elapsed());
            let first = d.slots[0];
            // DEVICES.md 15.3: an experimental driver has no inputs.
            if d.experimental.is_some() && reply.get(4..).is_some_and(|b| b.len() != 16 || b.iter().any(|&x| x != 0)) {
                let e = "an experimental driver gave inputs at BEGIN (DEVICES.md 15.3)".to_string();
                return Err(bus.fail(p, e));
            }
            let mut r = Reader::new(&reply);
            r.skip(4)?;
            inputs.charge += r.u32()?;
            let e = r.u32()?;
            for _ in 0..e {
                let b = r.take(4)?;
                let event = Event { kind: b[0], a: b[1], b: b[2], c: b[3] };
                inputs.events.push(PosEvent { event, position: r.u8()?, flags: r.u8()?, index: r.u16()? });
            }
            apply_writes(&mut r, mem.ram, None).map_err(|e| bus.fail(p, e))?;
            let record = r.blob()?;
            if !record.is_empty() {
                self.records.push((first, record.to_vec()));
            }
        }
        Ok(inputs)
    }

    fn call(&mut self, slot: u8, function: u8, args: [u32; 6], budget: i64, mem: GuestMem) -> Result<CallOutcome, String> {
        let unknown = Ok(CallOutcome { a0: ERR_UNKNOWN, a1: None, charge: 0 });
        let Some(p) = self.slot_proc[slot as usize] else { return unknown };
        self.touch(p);
        let to = Addr::new(self.instance, slot);
        let mut bus = self.bus.borrow_mut();
        let batching = bus.config.batching;
        let d = &mut bus.procs[p];
        let Some(e) = d.tables.get(&slot).and_then(|t| t[function as usize]) else { return unknown };
        // An answered call's base charge, also when a check fails (SPEC §charges).
        let base = e.base_extra();
        // DEVICES.md 5: the core's checks.
        let mut buf: (u32, u32) = (0, 0);
        if e.has_buffer() {
            let Some(len) = e.buffer_len(&args) else {
                d.stats.checked_out += 1;
                return Ok(CallOutcome { a0: e.len_bad, a1: None, charge: base });
            };
            let addr = args[e.buf_addr_arg as usize];
            let rom_ok = e.access & access::ROM != 0 && e.access & access::WRITE == 0;
            let placed = len == 0
                || ram_range(mem.ram.len(), addr, len as usize).is_some()
                || (rom_ok && readable(mem.rom, &[], addr, len as usize).is_some());
            if !placed {
                d.stats.checked_out += 1;
                return Ok(CallOutcome { a0: e.place_bad, a1: None, charge: base });
            }
            buf = (addr, len);
        }
        if e.check_arg != proto::NO_ARG && args[e.check_arg as usize] > e.check_max {
            d.stats.checked_out += 1;
            return Ok(CallOutcome { a0: e.place_bad, a1: None, charge: base });
        }
        let bytes: &[u8] = if e.has_buffer() && e.access & access::READ != 0 && buf.1 > 0 {
            readable(mem.rom, mem.ram, buf.0, buf.1 as usize).unwrap_or(&[])
        } else {
            &[]
        };
        if batching && e.batchable() {
            let (a0, a1, charge) = e.batch_outcome(&args, buf.1);
            proto::encode_call(&mut d.batch, to, function, &args, budget, bytes);
            d.batch_count += 1;
            d.stats.batched += 1;
            return Ok(CallOutcome { a0, a1, charge: base + charge });
        }
        // A round trip. On the socket the read buffer goes inline.
        let inline = match d.link {
            Link::Socket(_) => bytes,
            Link::Shm(_) => &[],
        };
        let start = Instant::now();
        let mut call = Vec::new();
        proto::encode_call(&mut call, to, function, &args, budget, inline);
        let reply = bus.request(p, to, msg::CALL, |w| {
            w.bytes(&proto::split(&call[4..]).unwrap().body[1..]);
        }, Some((mem.rom, &*mem.ram)))?;
        let mut r = Reader::new(&reply);
        r.skip(1)?;
        let flags = r.u8()?;
        r.skip(2)?;
        let (a0, a1, charge) = (r.u32()?, r.u32()?, r.u32()?);
        // DEVICES.md 15.3: an experimental device's charge is its table's,
        // for every call; a driver that says otherwise has broken the protocol.
        if bus.procs[p].experimental.is_some() {
            let declared = e.batch_outcome(&args, buf.1).2;
            if charge != declared {
                return Err(bus.fail(p, format!("function {function} charged {charge}, its table {declared} (DEVICES.md 15.3)")));
            }
        }
        let within = (e.has_buffer() && e.access & access::WRITE != 0).then_some((buf.0, buf.0.wrapping_add(buf.1)));
        apply_writes(&mut r, mem.ram, Some(within.unwrap_or((0, 0)))).map_err(|e| bus.fail(p, e))?;
        bus.procs[p].stats.calls.add(start.elapsed());
        Ok(CallOutcome { a0, a1: (flags & 1 != 0).then_some(a1), charge: base + charge })
    }

    fn end_frame(&mut self, frame: u32, stopped: bool, _mem: GuestMem) -> Result<(), String> {
        let mut bus = self.bus.borrow_mut();
        for &p in &self.procs {
            self.dirty.set(self.dirty.get() | 1 << p);
            bus.post(p, Addr::of(self.instance), msg::END, |w| {
                w.u8(stopped as u8).u16(0).u32(frame);
            })?;
        }
        Ok(())
    }

    fn write_state(&mut self, slot: u8, out: &mut Vec<u8>) -> Result<(), String> {
        out.extend_from_slice(&self.state(slot)?);
        Ok(())
    }

    fn restore_state(&mut self, slot: u8, bytes: &[u8], _ram_len: usize) -> Result<usize, RestoreError> {
        match self.restore(slot, bytes) {
            Ok(Some(n)) if n <= bytes.len() => Ok(n),
            Ok(_) => Err(bad("a device refused its section")),
            Err(_) => Err(bad("a device driver failed")),
        }
    }

    /// The sound driver's `sound.frame` (DEVICES.md 7.2), if it rendered a
    /// frame since the last call.
    fn take_audio(&mut self) -> Option<Vec<i16>> {
        self.slot_proc[terse_core::sound::SLOT as usize]?;
        let bytes = self.property(property::SOUND_FRAME).ok().flatten()?;
        let frame = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
        if self.audio_frame.replace(frame) == Some(frame) {
            return None;
        }
        Some(bytes[4..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect())
    }

    fn list3(&self) -> Cow<'_, [u8]> {
        if self.slot_proc[terse_core::vector3::SLOT as usize].is_none() {
            return Cow::Borrowed(&[]);
        }
        if let Some(l) = self.list.borrow().as_ref() {
            return Cow::Owned(l.clone());
        }
        let list = self.property(property::LIST3).ok().flatten().unwrap_or_default();
        *self.list.borrow_mut() = Some(list.clone());
        Cow::Owned(list)
    }

    fn new_machine(&self, serial: u32, image: &Image, grants: Option<&Grants>, identity: Option<&ProgramId>) -> Result<Machine<Self>, StartFailure> {
        Bus::open_instance(&self.bus, serial, image, grants, identity)
    }
}
