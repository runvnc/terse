//! The driver's side of the bus: connects, answers HELLO, keeps each
//! machine instance's memory, and turns the core's messages into calls on
//! a [`Device`]. A Rust driver implements `Device` and calls [`serve`];
//! drivers in other languages speak DEVICES.md themselves.

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use terse_core::Event;
use terse_core::content::ram_range;

use crate::proto::{self, Addr, CallEntry, DeviceInfo, Hello, Open, PosEvent, Reader, Writer, access, hello_flags, msg};
use crate::transport::{Mapping, ShmEnd, SocketEnd, Spin};

/// Guest memory as a driver reaches it: through the core (socket) or the
/// instance's memory file (shared memory).
pub trait Mem {
    /// The bytes `[addr, addr + len)` if they lie wholly in ROM or RAM.
    fn read(&mut self, addr: u32, len: usize) -> Option<Vec<u8>>;
    /// Writes RAM (within what the protocol allows the driver to write
    /// now).
    fn write(&mut self, addr: u32, bytes: &[u8]);
    fn ram_len(&self) -> usize;
}

/// A call as the driver gets it.
pub struct CallIn<'a> {
    pub slot: u8,
    pub function: u8,
    pub args: [u32; 6],
    pub budget: i64,
    /// The bytes of the read buffer, all of it.
    pub buf: &'a [u8],
    /// Part of a batch: no reply is sent.
    pub batched: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    pub a0: u32,
    pub a1: Option<u32>,
    pub charge: u32,
}

#[derive(Default)]
pub struct Inputs {
    pub charge: u32,
    pub events: Vec<PosEvent>,
    pub record: Vec<u8>,
}

/// BEGIN as the driver gets it.
pub struct Begin<'a> {
    pub frame: u32,
    pub time: u32,
    pub host: &'a [Event],
    pub replay: Option<&'a [u8]>,
}

/// PRESENT as the driver gets it (DEVICES.md 7).
pub struct Present {
    pub picture: (u32, u32),
    pub screen: (f32, f32, f32, f32),
    /// The glyph cell width to draw the terminal with (0: the driver's
    /// native cell, scaled).
    pub cell: u32,
}

/// One layer's answer to PRESENT.
pub struct LayerOut {
    pub slot: u8,
    pub id: u8,
    pub new_file: Option<PathBuf>,
    pub damage: Vec<[i32; 4]>,
}

#[derive(Default)]
pub struct PresentOut {
    pub layers: Vec<LayerOut>,
}

/// What the HELLO said that a device may want.
pub struct Session {
    pub hello: Hello,
}

impl Session {
    pub fn presents(&self) -> bool {
        self.hello.flags & hello_flags::PRESENTS != 0
    }

    pub fn replay(&self) -> bool {
        self.hello.flags & hello_flags::REPLAY != 0
    }

    pub fn dir(&self) -> &Path {
        Path::new(&self.hello.session_dir)
    }

    pub fn asks(&self, slot: u8) -> bool {
        self.hello.slots.contains(&slot)
    }
}

/// A device driver: one process, one or more slots (DEVICES.md 1.1), any
/// number of machine instances (1.2).
pub trait Device {
    /// The HELLO reply's description: name, slots (versions, layers, call
    /// tables), properties and services.
    fn info(&mut self, session: &Session) -> DeviceInfo;
    /// A machine instance begins (OPEN). An error refuses it.
    fn open(&mut self, instance: u32, open: &Open, session: &Session) -> Result<(), String>;
    /// The instance is gone (CLOSE).
    fn close(&mut self, instance: u32);
    fn reset(&mut self, instance: u32);
    fn call(&mut self, instance: u32, call: &CallIn, mem: &mut dyn Mem) -> Reply;
    fn begin(&mut self, instance: u32, begin: &Begin, mem: &mut dyn Mem) -> Inputs;
    fn end(&mut self, instance: u32, frame: u32, stopped: bool);
    fn state(&self, instance: u32, slot: u8) -> Vec<u8>;
    /// Restores `slot`'s section from the start of `bytes`; the section's
    /// length, or `None` if it is not a valid section (nothing changed).
    fn restore(&mut self, instance: u32, slot: u8, bytes: &[u8]) -> Option<usize>;
    fn present(&mut self, _instance: u32, _present: &Present, _session: &Session) -> PresentOut {
        PresentOut::default()
    }
    /// A property it publishes (None: not one of them).
    fn get(&mut self, _instance: u32, _name: &str) -> Option<Vec<u8>> {
        None
    }
    /// A property it subscribes to changed.
    fn set(&mut self, _instance: u32, _name: &str, _value: &[u8]) {}
    /// A host service (DEVICES.md 9).
    fn service(&mut self, _instance: u32, name: &str, _request: &[u8]) -> Result<Vec<u8>, String> {
        Err(format!("no service {name}"))
    }
}

/// Guest memory through the socket: ROM from OPEN, RAM by READ; writes go
/// back in the reply.
struct SocketMem<'a> {
    end: &'a mut SocketEnd,
    to: Addr,
    rom: &'a [u8],
    ram_len: usize,
    writes: &'a mut Vec<(u32, Vec<u8>)>,
    buf: Vec<u8>,
}

impl Mem for SocketMem<'_> {
    fn read(&mut self, addr: u32, len: usize) -> Option<Vec<u8>> {
        if len == 0 {
            return Some(Vec::new());
        }
        let off = addr.wrapping_sub(terse_core::ROM_BASE) as usize;
        if off <= self.rom.len() && self.rom.len() - off >= len {
            return Some(self.rom[off..off + len].to_vec());
        }
        ram_range(self.ram_len, addr, len)?;
        let mut out = Vec::new();
        let mut w = Writer::new(&mut out, self.to, msg::READ);
        w.u8(0).u16(0).u32(addr).u32(len as u32);
        w.finish();
        self.end.send(&out).ok()?;
        self.end.recv(&mut self.buf).ok()??;
        let m = proto::split(&self.buf).ok()?;
        let mut r = Reader::new(m.body);
        (r.u8().ok()? == msg::DATA).then_some(())?;
        let ok = r.u8().ok()?;
        r.skip(2).ok()?;
        (ok == 1).then(|| r.rest().to_vec())
    }

    fn write(&mut self, addr: u32, bytes: &[u8]) {
        self.writes.push((addr, bytes.to_vec()));
    }

    fn ram_len(&self) -> usize {
        self.ram_len
    }
}

/// Guest memory in the instance's memory file.
struct ShmMem<'a> {
    map: &'a mut Mapping,
    rom_len: usize,
    ram_len: usize,
}

impl Mem for ShmMem<'_> {
    fn read(&mut self, addr: u32, len: usize) -> Option<Vec<u8>> {
        let b = self.map.bytes();
        let (rom, ram) = (&b[..self.rom_len], &b[proto::MEMORY_RAM_OFFSET..][..self.ram_len]);
        terse_core::content::readable(rom, ram, addr, len).map(<[u8]>::to_vec)
    }

    fn write(&mut self, addr: u32, bytes: &[u8]) {
        if let Some(at) = ram_range(self.ram_len, addr, bytes.len()) {
            self.map.bytes_mut()[proto::MEMORY_RAM_OFFSET + at..][..bytes.len()].copy_from_slice(bytes);
        }
    }

    fn ram_len(&self) -> usize {
        self.ram_len
    }
}

enum DriverLink {
    Socket(SocketEnd),
    Shm(ShmEnd),
}

/// What the driver library keeps of an instance.
struct Instance {
    rom: Vec<u8>,
    ram_len: usize,
    memory: Option<Mapping>,
}

/// The spin of a driver waiting for the core's next request: up to 2 ms
/// (`TERSE_BUS_SPIN_US`), adaptive unless `TERSE_BUS_SPIN=fixed`.
fn driver_spin() -> Spin {
    let max = std::env::var("TERSE_BUS_SPIN_US").ok().and_then(|s| s.parse().ok()).map_or(Duration::from_millis(2), Duration::from_micros);
    Spin { max, adaptive: !std::env::var("TERSE_BUS_SPIN").is_ok_and(|s| s == "fixed") }
}

/// Connects to the core (TERSE_BUS_SOCKET), answers HELLO with `device`'s
/// description, and serves until SHUTDOWN or the core goes. The transport:
/// shared memory if offered, unless TERSE_BUS_TRANSPORT=socket.
pub fn serve(device: &mut dyn Device) -> Result<(), String> {
    let path = std::env::var("TERSE_BUS_SOCKET").map_err(|_| "TERSE_BUS_SOCKET is not set: a driver is started by a core (terse-bus)".to_string())?;
    if let Some(cpu) = std::env::var("TERSE_BUS_CPU").ok().and_then(|c| c.parse().ok()) {
        crate::transport::pin_to(cpu);
    }
    let want_socket = std::env::var("TERSE_BUS_TRANSPORT").is_ok_and(|t| t == "socket");
    let stream = UnixStream::connect(&path).map_err(|e| format!("{path}: {e}"))?;
    let mut sock = SocketEnd::new(stream)?;
    let mut buf = Vec::new();
    sock.recv(&mut buf)?.ok_or("the core closed the socket")?;
    let hello = Hello::decode(proto::split(&buf)?.body)?;
    let session = Session { hello };
    let mut info = device.info(&session);
    // Its identity (DEVICES.md 1.1, PERFORMANCE.md R13): every Rust driver
    // carries the workspace's version and its executable's build.
    if info.version.is_empty() {
        info.version = terse_perflog::identity::VERSION.to_string();
    }
    if info.build.is_empty() {
        info.build = terse_perflog::identity::build_id().to_string();
    }
    let shm = session.hello.flags & hello_flags::SHM != 0 && !want_socket;
    // Shared memory: the bus file is mapped, and the core's sequence
    // number taken, before the reply, after which the core may send its
    // first request (DEVICES.md 4.2).
    let mapped = if shm {
        let map = Mapping::open(Path::new(&session.hello.bus_path), true)?;
        let socket = sock.stream.try_clone().map_err(|e| e.to_string())?;
        Some(ShmEnd::new(map, false, driver_spin(), socket)?)
    } else {
        None
    };
    let mut out = Vec::new();
    info.encode(if shm { proto::TRANSPORT_SHM } else { proto::TRANSPORT_SOCKET }, &mut out);
    sock.send(&out)?;
    let mut tables: HashMap<u8, Vec<Option<CallEntry>>> = HashMap::new();
    for s in &info.slots {
        let mut table = vec![None; 256];
        for e in &s.table {
            table[e.function as usize] = Some(*e);
        }
        tables.insert(s.slot, table);
    }
    let mut link = match mapped {
        Some(end) => DriverLink::Shm(end),
        None => DriverLink::Socket(sock),
    };
    let mut server = Server { tables, session, instances: HashMap::new(), writes: Vec::new(), out: Vec::new() };
    let mut packet = Vec::new();
    loop {
        match &mut link {
            DriverLink::Socket(end) => {
                if end.recv(&mut packet)?.is_none() {
                    return Ok(());
                }
                let m = std::mem::take(&mut packet);
                let done = server.handle(device, proto::split(&m)?, &mut link)?;
                packet = m;
                if done {
                    return Ok(());
                }
            }
            DriverLink::Shm(end) => {
                let Ok(p) = end.recv() else { return Ok(()) };
                packet.clear();
                packet.extend_from_slice(p);
                let all: Vec<Vec<u8>> = proto::messages(&packet).map(|m| m.map(|m| [&m.to.instance.to_le_bytes()[..], &[m.to.slot, 0, 0, 0], m.body].concat())).collect::<Result<_, _>>()?;
                for m in all {
                    if server.handle(device, proto::split(&m)?, &mut link)? {
                        return Ok(());
                    }
                }
            }
        }
    }
}

struct Server {
    tables: HashMap<u8, Vec<Option<CallEntry>>>,
    session: Session,
    instances: HashMap<u32, Instance>,
    writes: Vec<(u32, Vec<u8>)>,
    out: Vec<u8>,
}

impl Server {
    fn send(&mut self, link: &mut DriverLink) -> Result<(), String> {
        let out = std::mem::take(&mut self.out);
        let r = match link {
            DriverLink::Socket(end) => end.send(&out),
            DriverLink::Shm(end) => end.send(&out),
        };
        self.out = out;
        r
    }

    /// Runs `f` with the guest memory of instance `to`.
    fn with_mem<T>(&mut self, link: &mut DriverLink, to: Addr, f: impl FnOnce(&mut dyn Mem) -> T) -> Result<T, String> {
        let inst = self.instances.get_mut(&to.instance).ok_or_else(|| format!("no instance {}", to.instance))?;
        let (rom, ram_len) = (&inst.rom, inst.ram_len);
        Ok(match (link, &mut inst.memory) {
            (_, Some(memory)) => f(&mut ShmMem { map: memory, rom_len: rom.len(), ram_len }),
            (DriverLink::Socket(end), None) => f(&mut SocketMem { end, to, rom, ram_len, writes: &mut self.writes, buf: Vec::new() }),
            (DriverLink::Shm(_), None) => return Err("shared memory without a memory file".into()),
        })
    }

    fn error(&mut self, link: &mut DriverLink, to: Addr, text: &str) -> Result<(), String> {
        self.out.clear();
        Writer::new(&mut self.out, to, msg::ERROR).bytes(text.as_bytes()).finish();
        self.send(link)
    }

    fn write_writes(w: &mut Writer, writes: &[(u32, Vec<u8>)]) {
        w.u32(writes.len() as u32);
        for (addr, b) in writes {
            w.u32(*addr).blob(b);
        }
    }

    /// One message; true once the driver should exit.
    fn handle(&mut self, device: &mut dyn Device, m: proto::Msg, link: &mut DriverLink) -> Result<bool, String> {
        let (to, body) = (m.to, m.body);
        let Some(&kind) = body.first() else { return Err("an empty message".into()) };
        self.out.clear();
        self.writes.clear();
        let known = |s: &Self| s.instances.contains_key(&to.instance);
        // A host service may be asked on behalf of a machine that has none
        // of the driver's devices (a shell without device 4 starting
        // machines, DEVICES.md 1.9): the device decides what it needs of it.
        if !matches!(kind, msg::OPEN | msg::BATCH | msg::SHUTDOWN | msg::ERROR | msg::CLOSE | msg::SERVICE) && !known(self) {
            self.error(link, to, &format!("no instance {}", to.instance))?;
            return Ok(false);
        }
        match kind {
            msg::OPEN => {
                let open = Open::decode(body)?;
                let memory = match open.memory_path.as_str() {
                    "" => None,
                    p => Some(Mapping::open(Path::new(p), true)?),
                };
                if matches!(link, DriverLink::Shm(_)) && memory.is_none() {
                    return Err("shared memory without a memory file".into());
                }
                let result = device.open(to.instance, &open, &self.session);
                if result.is_ok() {
                    self.instances.insert(to.instance, Instance { rom: open.rom.clone(), ram_len: open.ram_len as usize, memory });
                }
                let mut w = Writer::new(&mut self.out, to, msg::OPEN | msg::REPLY);
                match &result {
                    Ok(()) => w.u8(1).u16(0).str(&[]),
                    Err(e) => w.u8(0).u16(0).str(e.as_bytes()),
                };
                w.finish();
                self.send(link)?;
            }
            msg::CLOSE => {
                if self.instances.remove(&to.instance).is_some() {
                    device.close(to.instance);
                }
            }
            msg::CALL => {
                let reply = self.call(device, to, body, link, false)?;
                let mut w = Writer::new(&mut self.out, to, msg::RESULT);
                w.u8(reply.a1.is_some() as u8).u16(0).u32(reply.a0).u32(reply.a1.unwrap_or(0)).u32(reply.charge);
                Self::write_writes(&mut w, &self.writes);
                w.finish();
                self.send(link)?;
            }
            msg::BATCH => {
                let mut r = Reader::new(body);
                r.skip(4)?;
                let n = r.u32()?;
                for _ in 0..n {
                    let call = proto::split(r.blob()?)?;
                    if !known(self) && !self.instances.contains_key(&call.to.instance) {
                        return Err(format!("a batched call for no instance {}", call.to.instance));
                    }
                    let reply = self.call(device, call.to, call.body, link, true)?;
                    #[cfg(debug_assertions)]
                    {
                        let c = proto::decode_call(call.body)?;
                        if let Some(e) = self.tables.get(&call.to.slot).and_then(|t| t[c.function as usize]) {
                            let (a0, a1, charge) = e.batch_outcome(&c.args, buffer_len(&e, &c.args));
                            debug_assert_eq!((reply.a0, reply.a1, reply.charge), (a0, a1, charge), "batched call {:02x}", c.function);
                        }
                    }
                    let _ = reply;
                }
            }
            msg::BEGIN => {
                let mut r = Reader::new(body);
                r.skip(1)?;
                let flags = r.u8()?;
                r.skip(2)?;
                let (frame, time, e) = (r.u32()?, r.u32()?, r.u32()?);
                let mut host = Vec::with_capacity(e as usize);
                for _ in 0..e {
                    let b = r.take(4)?;
                    host.push(Event { kind: b[0], a: b[1], b: b[2], c: b[3] });
                }
                let record = r.blob()?;
                let begin = Begin { frame, time, host: &host, replay: (flags & 1 != 0).then_some(record) };
                let inputs = self.with_mem(link, to, |m| device.begin(to.instance, &begin, m))?;
                let mut w = Writer::new(&mut self.out, to, msg::BEGIN | msg::REPLY);
                w.u8(0).u16(0).u32(inputs.charge).u32(inputs.events.len() as u32);
                for e in &inputs.events {
                    w.bytes(&[e.event.kind, e.event.a, e.event.b, e.event.c, e.position, e.flags]).u16(e.index);
                }
                Self::write_writes(&mut w, &self.writes);
                w.blob(&inputs.record);
                w.finish();
                self.send(link)?;
            }
            msg::END => {
                let mut r = Reader::new(body);
                r.skip(1)?;
                let stopped = r.u8()? != 0;
                r.skip(2)?;
                device.end(to.instance, r.u32()?, stopped);
            }
            msg::STATE => {
                let s = device.state(to.instance, to.slot);
                Writer::new(&mut self.out, to, msg::STATE | msg::REPLY).u8(0).u16(0).blob(&s).finish();
                self.send(link)?;
            }
            msg::RESTORE => {
                let mut r = Reader::new(body);
                r.skip(4)?;
                let taken = device.restore(to.instance, to.slot, r.blob()?);
                Writer::new(&mut self.out, to, msg::RESTORE | msg::REPLY).u8(taken.is_some() as u8).u16(0).u32(taken.unwrap_or(0) as u32).finish();
                self.send(link)?;
            }
            msg::RESET => {
                device.reset(to.instance);
                Writer::new(&mut self.out, to, msg::RESET | msg::REPLY).finish();
                self.send(link)?;
            }
            msg::GET => {
                let mut r = Reader::new(body);
                r.skip(2)?;
                let n = r.u8()?;
                let mut values = Vec::new();
                for _ in 0..n {
                    let name = r.string()?;
                    if let Some(v) = device.get(to.instance, &name) {
                        values.push((name, v));
                    }
                }
                let mut w = Writer::new(&mut self.out, to, msg::GET | msg::REPLY);
                w.u8(values.len() as u8);
                for (name, v) in &values {
                    w.str(name.as_bytes()).blob(v);
                }
                w.finish();
                self.send(link)?;
            }
            msg::SET => {
                let mut r = Reader::new(body);
                r.skip(2)?;
                let name = r.string()?;
                device.set(to.instance, &name, r.blob()?);
            }
            msg::SERVICE => {
                let mut r = Reader::new(body);
                r.skip(2)?;
                let name = r.string()?;
                let result = device.service(to.instance, &name, r.blob()?);
                let mut w = Writer::new(&mut self.out, to, msg::SERVICE | msg::REPLY);
                match &result {
                    Ok(v) => w.u8(1).str(&[]).blob(v),
                    Err(e) => w.u8(0).str(e.as_bytes()).blob(&[]),
                };
                w.finish();
                self.send(link)?;
            }
            msg::PRESENT => {
                let mut r = Reader::new(body);
                r.skip(4)?;
                let picture = (r.u16()? as u32, r.u16()? as u32);
                let screen = (r.f32()?, r.f32()?, r.f32()?, r.f32()?);
                let cell = r.u16()? as u32;
                let out = device.present(to.instance, &Present { picture, screen, cell }, &self.session);
                let mut w = Writer::new(&mut self.out, to, msg::PRESENT | msg::REPLY);
                w.u8(out.layers.len() as u8).u16(0);
                for l in &out.layers {
                    w.u8(l.slot).u8(l.id).u8(l.new_file.is_some() as u8);
                    if let Some(p) = &l.new_file {
                        w.str(p.display().to_string().as_bytes());
                    }
                    w.u32(l.damage.len() as u32);
                    for d in &l.damage {
                        w.i32(d[0]).i32(d[1]).i32(d[2]).i32(d[3]);
                    }
                }
                w.finish();
                self.send(link)?;
            }
            msg::SHUTDOWN => return Ok(true),
            msg::ERROR => return Err(format!("the core: {}", String::from_utf8_lossy(&body[1..]))),
            other => self.error(link, to, &format!("unknown message {other:02x}"))?,
        }
        Ok(false)
    }

    fn call(&mut self, device: &mut dyn Device, to: Addr, body: &[u8], link: &mut DriverLink, batched: bool) -> Result<Reply, String> {
        let c = proto::decode_call(body)?;
        let entry = self.tables.get(&to.slot).and_then(|t| t[c.function as usize]);
        // The read buffer: inline, or in the memory file.
        let mut from_memory = Vec::new();
        if c.inline.is_empty()
            && let Some(e) = entry
            && e.has_buffer()
            && e.access & access::READ != 0
            && let Some(Instance { rom, ram_len, memory: Some(memory) }) = self.instances.get(&to.instance)
        {
            let len = buffer_len(&e, &c.args) as usize;
            let addr = c.args[e.buf_addr_arg as usize];
            let b = memory.bytes();
            let ram = &b[proto::MEMORY_RAM_OFFSET..][..*ram_len];
            if let Some(bytes) = terse_core::content::readable(&b[..rom.len()], ram, addr, len) {
                from_memory.extend_from_slice(bytes);
            }
        }
        let buf: &[u8] = if c.inline.is_empty() { &from_memory } else { c.inline };
        let call = CallIn { slot: to.slot, function: c.function, args: c.args, budget: c.budget, buf, batched };
        self.with_mem(link, to, |m| device.call(to.instance, &call, m))
    }
}

fn buffer_len(e: &CallEntry, args: &[u32; 6]) -> u32 {
    e.buffer_len(args).unwrap_or(0)
}

/// A layer's pixels in a file of the session directory (DEVICES.md 7):
/// premultiplied, alpha in the top byte.
pub struct LayerFile {
    pub path: PathBuf,
    pub map: Mapping,
    pub size: (u32, u32),
}

impl LayerFile {
    pub fn create(dir: &Path, instance: u32, slot: u8, id: u8, size: (u32, u32)) -> Result<Self, String> {
        let mut seed = [0u8; 4];
        let _ = getrandom::fill(&mut seed);
        let path = dir.join(format!("layer-{instance}-{slot}-{id}-{}x{}-{:08x}", size.0, size.1, u32::from_le_bytes(seed)));
        let map = Mapping::create(&path, (size.0 * size.1 * 4).max(4) as usize)?;
        Ok(LayerFile { path, map, size })
    }

    pub fn pixels(&mut self) -> &mut [u32] {
        let n = (self.size.0 * self.size.1) as usize;
        let b = self.map.bytes_mut();
        // SAFETY: the mapping is page aligned and n * 4 bytes long at least.
        unsafe { std::slice::from_raw_parts_mut(b.as_mut_ptr() as *mut u32, n) }
    }
}

impl Drop for LayerFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// `--version` among the arguments: prints the driver's identity (`name`,
/// the version, the build identifier: PERFORMANCE.md R13) and returns
/// true; the driver then exits without connecting.
pub fn version(name: &str) -> bool {
    terse_perflog::identity::print_if_asked(name, &std::env::args().skip(1).collect::<Vec<_>>())
}

/// Exits the process with a message if `serve` fails.
pub fn run(device: &mut dyn Device, name: &str) -> std::process::ExitCode {
    if version(name) {
        return std::process::ExitCode::SUCCESS;
    }
    match serve(device) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{name}: {e}");
            std::process::ExitCode::from(1)
        }
    }
}
