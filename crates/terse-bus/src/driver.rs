//! The driver's side of the bus: connects, answers HELLO, and turns the
//! core's messages into calls on a [`Device`]. A Rust driver implements
//! `Device` and calls [`serve`]; drivers in other languages speak
//! DEVICES.md themselves.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use terse_core::Event;
use terse_core::content::ram_range;

use crate::proto::{self, CallEntry, DeviceInfo, Hello, PosEvent, Reader, Writer, access, hello_flags, msg};
use crate::transport::{Mapping, ShmEnd, SocketEnd};

/// Guest memory as a driver reaches it: through the core (socket) or the
/// memory file (shared memory).
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
    pub function: u8,
    pub args: [u32; 6],
    pub budget: i64,
    /// The bytes of the read buffer (all of it, or for a budget-bounded
    /// buffer at least as many as the budget allows).
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

/// PRESENT as the driver gets it.
pub struct Present {
    pub picture: (u32, u32),
    pub screen: (f32, f32, f32, f32),
    /// (slot, key, value).
    pub properties: Vec<(u8, u8, u32)>,
}

/// One layer's answer to PRESENT.
pub struct LayerOut {
    pub id: u8,
    pub new_file: Option<PathBuf>,
    pub damage: Vec<[i32; 4]>,
}

#[derive(Default)]
pub struct PresentOut {
    pub layers: Vec<LayerOut>,
    pub properties: Vec<(u8, u32)>,
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
}

pub trait Device {
    /// The HELLO reply's description: version, name, layers, call table.
    fn info(&mut self, session: &Session) -> DeviceInfo;
    fn reset(&mut self);
    fn call(&mut self, call: &CallIn, mem: &mut dyn Mem) -> Reply;
    fn begin(&mut self, begin: &Begin, mem: &mut dyn Mem) -> Inputs;
    fn end(&mut self, frame: u32, stopped: bool);
    fn state(&self) -> Vec<u8>;
    fn restore(&mut self, bytes: &[u8]) -> bool;
    fn present(&mut self, _present: &Present, _session: &Session) -> PresentOut {
        PresentOut::default()
    }
}

/// Guest memory through the socket: ROM from HELLO, RAM by READ; writes
/// go back in the reply.
struct SocketMem<'a> {
    end: &'a mut SocketEnd,
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
        let mut w = Writer::new(&mut out, msg::READ);
        w.u8(0).u16(0).u32(addr).u32(len as u32);
        w.finish();
        self.end.send(&out).ok()?;
        self.end.recv(&mut self.buf).ok()??;
        let mut r = Reader::new(&self.buf);
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

/// Guest memory in the memory file.
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
    Shm { end: ShmEnd, memory: Mapping },
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
    let spin = std::env::var("TERSE_BUS_SPIN_US").ok().and_then(|s| s.parse().ok()).map_or(Duration::from_millis(2), Duration::from_micros);
    let stream = UnixStream::connect(&path).map_err(|e| format!("{path}: {e}"))?;
    let mut sock = SocketEnd::new(stream)?;
    let mut buf = Vec::new();
    sock.recv(&mut buf)?.ok_or("the core closed the socket")?;
    let hello = Hello::decode(&buf)?;
    let session = Session { hello };
    let info = device.info(&session);
    let shm = session.hello.flags & hello_flags::SHM != 0 && !want_socket;
    let mut out = Vec::new();
    info.encode(session.hello.slot, if shm { proto::TRANSPORT_SHM } else { proto::TRANSPORT_SOCKET }, &mut out);
    sock.send(&out)?;
    let mut table: Vec<Option<CallEntry>> = vec![None; 256];
    for e in &info.table {
        table[e.function as usize] = Some(*e);
    }
    let mut link = if shm {
        let map = Mapping::open(Path::new(&session.hello.bus_path), true)?;
        let memory = Mapping::open(Path::new(&session.hello.memory_path), true)?;
        DriverLink::Shm { end: ShmEnd::new(map, false, spin, sock.stream)?, memory }
    } else {
        DriverLink::Socket(sock)
    };
    let mut server = Server { table, session, writes: Vec::new(), out: Vec::new() };
    let mut packet = Vec::new();
    loop {
        match &mut link {
            DriverLink::Socket(end) => {
                if end.recv(&mut packet)?.is_none() {
                    return Ok(());
                }
                let body = std::mem::take(&mut packet);
                let done = server.handle(device, &body, &mut link)?;
                packet = body;
                if done {
                    return Ok(());
                }
            }
            DriverLink::Shm { end, .. } => {
                let Some(p) = end.recv() else { return Ok(()) };
                packet.clear();
                packet.extend_from_slice(p);
                let bodies: Vec<std::ops::Range<usize>> = {
                    let mut v = Vec::new();
                    let mut at = 0;
                    while at + 4 <= packet.len() {
                        let n = proto::u32_at(&packet, at) as usize;
                        v.push(at + 4..(at + 4 + n).min(packet.len()));
                        at += 4 + n;
                    }
                    v
                };
                for r in bodies {
                    let body = packet[r].to_vec();
                    if server.handle(device, &body, &mut link)? {
                        return Ok(());
                    }
                }
            }
        }
    }
}

struct Server {
    table: Vec<Option<CallEntry>>,
    session: Session,
    writes: Vec<(u32, Vec<u8>)>,
    out: Vec<u8>,
}

impl Server {
    fn send(&mut self, link: &mut DriverLink) -> Result<(), String> {
        let out = std::mem::take(&mut self.out);
        let r = match link {
            DriverLink::Socket(end) => end.send(&out),
            DriverLink::Shm { end, .. } => end.send(&out),
        };
        self.out = out;
        r
    }

    /// Runs `f` with the guest memory of the link.
    fn with_mem<T>(&mut self, link: &mut DriverLink, f: impl FnOnce(&mut dyn Mem) -> T) -> T {
        let rom = &self.session.hello.rom;
        let ram_len = self.session.hello.ram_len as usize;
        match link {
            DriverLink::Socket(end) => {
                let mut m = SocketMem { end, rom, ram_len, writes: &mut self.writes, buf: Vec::new() };
                f(&mut m)
            }
            DriverLink::Shm { memory, .. } => {
                let mut m = ShmMem { map: memory, rom_len: rom.len(), ram_len };
                f(&mut m)
            }
        }
    }

    /// One message; true once the driver should exit.
    fn handle(&mut self, device: &mut dyn Device, body: &[u8], link: &mut DriverLink) -> Result<bool, String> {
        let Some(&kind) = body.first() else { return Err("an empty message".into()) };
        self.out.clear();
        self.writes.clear();
        match kind {
            msg::CALL => {
                let reply = self.call(device, body, link, false)?;
                let mut w = Writer::new(&mut self.out, msg::RESULT);
                w.u8(reply.a1.is_some() as u8).u16(0).u32(reply.a0).u32(reply.a1.unwrap_or(0)).u32(reply.charge);
                w.u32(self.writes.len() as u32);
                for (addr, b) in &self.writes {
                    w.u32(*addr).blob(b);
                }
                w.finish();
                self.send(link)?;
            }
            msg::BATCH => {
                let mut r = Reader::new(body);
                r.skip(4)?;
                let n = r.u32()?;
                for _ in 0..n {
                    let call = r.blob()?;
                    let reply = self.call(device, call, link, true)?;
                    #[cfg(debug_assertions)]
                    {
                        let c = proto::decode_call(call)?;
                        if let Some(e) = self.table[c.function as usize] {
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
                let inputs = self.with_mem(link, |m| device.begin(&begin, m));
                let mut w = Writer::new(&mut self.out, msg::BEGIN | msg::REPLY);
                w.u8(0).u16(0).u32(inputs.charge).u32(inputs.events.len() as u32);
                for e in &inputs.events {
                    w.bytes(&[e.event.kind, e.event.a, e.event.b, e.event.c, e.position, e.flags]).u16(e.index);
                }
                w.u32(self.writes.len() as u32);
                for (addr, b) in &self.writes {
                    w.u32(*addr).blob(b);
                }
                w.blob(&inputs.record);
                w.finish();
                self.send(link)?;
            }
            msg::END => {
                let mut r = Reader::new(body);
                r.skip(1)?;
                let stopped = r.u8()? != 0;
                r.skip(2)?;
                device.end(r.u32()?, stopped);
            }
            msg::STATE => {
                let s = device.state();
                Writer::new(&mut self.out, msg::STATE | msg::REPLY).u8(0).u16(0).blob(&s).finish();
                self.send(link)?;
            }
            msg::RESTORE => {
                let mut r = Reader::new(body);
                r.skip(4)?;
                let ok = device.restore(r.blob()?);
                Writer::new(&mut self.out, msg::RESTORE | msg::REPLY).u8(ok as u8).u16(0).finish();
                self.send(link)?;
            }
            msg::RESET => {
                device.reset();
                Writer::new(&mut self.out, msg::RESET | msg::REPLY).finish();
                self.send(link)?;
            }
            msg::PRESENT => {
                let mut r = Reader::new(body);
                r.skip(4)?;
                let picture = (r.u16()? as u32, r.u16()? as u32);
                let screen = (r.f32()?, r.f32()?, r.f32()?, r.f32()?);
                let p = r.u32()?;
                let mut properties = Vec::new();
                for _ in 0..p {
                    let (s, k) = (r.u8()?, r.u8()?);
                    r.skip(2)?;
                    properties.push((s, k, r.u32()?));
                }
                let out = device.present(&Present { picture, screen, properties }, &self.session);
                let mut w = Writer::new(&mut self.out, msg::PRESENT | msg::REPLY);
                w.u8(out.layers.len() as u8).u16(0);
                for l in &out.layers {
                    w.u8(l.id).u8(l.new_file.is_some() as u8);
                    if let Some(p) = &l.new_file {
                        w.str(p.display().to_string().as_bytes());
                    }
                    w.u32(l.damage.len() as u32);
                    for d in &l.damage {
                        w.i32(d[0]).i32(d[1]).i32(d[2]).i32(d[3]);
                    }
                }
                w.u32(out.properties.len() as u32);
                for (k, v) in &out.properties {
                    w.u8(*k).u8(0).u16(0).u32(*v);
                }
                w.finish();
                self.send(link)?;
            }
            msg::SHUTDOWN => return Ok(true),
            msg::ERROR => return Err(format!("the core: {}", String::from_utf8_lossy(&body[1..]))),
            other => {
                Writer::new(&mut self.out, msg::ERROR).bytes(format!("unknown message {other:02x}").as_bytes()).finish();
                self.send(link)?;
            }
        }
        Ok(false)
    }

    fn call(&mut self, device: &mut dyn Device, body: &[u8], link: &mut DriverLink, batched: bool) -> Result<Reply, String> {
        let c = proto::decode_call(body)?;
        let entry = self.table[c.function as usize];
        // The read buffer: inline, or in the memory file.
        let mut from_memory = Vec::new();
        if c.inline.is_empty()
            && let (Some(e), DriverLink::Shm { memory, .. }) = (entry, &*link)
            && e.has_buffer()
            && e.access & access::READ != 0
        {
            let len = buffer_len(&e, &c.args) as usize;
            let addr = c.args[e.buf_addr_arg as usize];
            let b = memory.bytes();
            let rom = &b[..self.session.hello.rom.len()];
            let ram = &b[proto::MEMORY_RAM_OFFSET..][..self.session.hello.ram_len as usize];
            if let Some(bytes) = terse_core::content::readable(rom, ram, addr, len) {
                from_memory.extend_from_slice(bytes);
            }
        }
        let buf: &[u8] = if c.inline.is_empty() { &from_memory } else { c.inline };
        let call = CallIn { function: c.function, args: c.args, budget: c.budget, buf, batched };
        Ok(self.with_mem(link, |m| device.call(&call, m)))
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
    pub fn create(dir: &Path, slot: u8, id: u8, size: (u32, u32)) -> Result<Self, String> {
        let mut seed = [0u8; 4];
        let _ = getrandom::fill(&mut seed);
        let path = dir.join(format!("layer-{slot}-{id}-{}x{}-{:08x}", size.0, size.1, u32::from_le_bytes(seed)));
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

/// Exits the process with a message if `serve` fails.
pub fn run(device: &mut dyn Device, name: &str) -> std::process::ExitCode {
    match serve(device) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{name}: {e}");
            std::process::ExitCode::from(1)
        }
    }
}
