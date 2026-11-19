//! The device bus seen from the core (DEVICES.md): driver processes
//! started and connected, the call table's checks, batching, BEGIN and
//! END, state, and presentation requests. Implements [`Devices`].

use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use terse_core::content::{ram_range, readable};
use terse_core::{ERR_UNKNOWN, Event};

use crate::core::{CallOutcome, Devices, FrameInputs, GuestMem};
use crate::proto::{self, CallEntry, DeviceInfo, Hello, LayerDecl, PosEvent, Reader, Writer, access, hello_flags, msg};
use crate::stats::Histogram;
use crate::transport::{self, Mapping, ShmEnd, SocketEnd};

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

    /// The reply's body into `buf`.
    fn recv(&mut self, buf: &mut Vec<u8>) -> Result<(), String> {
        match self {
            Link::Socket(s) => s.recv(buf)?.ok_or_else(|| "the driver closed its socket".to_string()),
            Link::Shm(s) => {
                let packet = s.recv().ok_or("the driver has gone")?;
                let mut msgs = proto::messages(packet);
                let body = msgs.next().ok_or("an empty reply")?.map_err(String::from)?;
                buf.clear();
                buf.extend_from_slice(body);
                Ok(())
            }
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
    /// Batches sent, and the calls in them.
    pub batches: u64,
}

pub struct Driver {
    pub slot: u8,
    pub info: DeviceInfo,
    pub transport: u8,
    table: Vec<Option<CallEntry>>,
    link: Link,
    child: Option<Child>,
    /// The batch: CALL messages (framed), and their count.
    batch: Vec<u8>,
    batch_count: u32,
    /// END held for shared memory until the next request.
    held: Vec<u8>,
    out: Vec<u8>,
    reply: Vec<u8>,
    pub stats: DriverStats,
    /// The command line, for messages.
    pub command: String,
}

/// How to start the bus.
pub struct BusConfig {
    pub session: PathBuf,
    /// Offer shared memory (the memory file exists).
    pub shm: bool,
    pub presents: bool,
    pub replay: bool,
    /// Batch calls that allow it.
    pub batching: bool,
    /// Spin time of the core's waits for replies on shared memory.
    pub spin: Duration,
    /// Pin each driver to a CPU (slot order), if given.
    pub driver_cpus: Vec<usize>,
}

pub struct Bus {
    pub drivers: Vec<Driver>,
    by_slot: [Option<usize>; 256],
    batching: bool,
    /// Gathered in `begin_frame`, applied by the core.
    scratch: Vec<u8>,
    /// Whether this frame's batches are flushed (with END).
    pub session: PathBuf,
}

/// Splits a command line at whitespace (no quoting: the prototype's
/// commands are paths and options).
pub fn split_command(cmd: &str) -> Vec<String> {
    cmd.split_whitespace().map(str::to_string).collect()
}

impl Bus {
    /// Starts and connects every driver (`(slot, version, command)`), in
    /// slot order. `rom` and the RAM length go in HELLO.
    pub fn start(
        config: &BusConfig,
        drivers: &[(u8, u8, Vec<String>)],
        image_id: [u8; 32],
        rom: &[u8],
        ram_len: u32,
        memory_path: Option<&Path>,
    ) -> Result<Bus, String> {
        let mut bus = Bus { drivers: Vec::new(), by_slot: [None; 256], batching: config.batching, scratch: Vec::new(), session: config.session.clone() };
        for (i, (slot, version, command)) in drivers.iter().enumerate() {
            let sock = config.session.join(format!("slot{slot}.sock"));
            let listener = UnixListener::bind(&sock).map_err(|e| format!("{}: {e}", sock.display()))?;
            let (program, args) = command.split_first().ok_or(format!("slot {slot}: an empty command"))?;
            let mut cmd = Command::new(program);
            cmd.args(args).env("TERSE_BUS_SOCKET", &sock).env("TERSE_BUS_SLOT", slot.to_string()).stdin(Stdio::null());
            if let Some(cpu) = config.driver_cpus.get(i) {
                cmd.env("TERSE_BUS_CPU", cpu.to_string());
            }
            let child = cmd.spawn().map_err(|e| format!("slot {slot}: cannot start {program}: {e}"))?;
            let mut driver_child = Some(child);
            let stream = match accept(&listener, Duration::from_secs(10), driver_child.as_mut().unwrap()) {
                Ok(s) => s,
                Err(e) => {
                    reap(&mut driver_child);
                    return Err(format!("slot {slot} ({program}): {e}"));
                }
            };
            let _ = std::fs::remove_file(&sock);
            let bus_path = config.session.join(format!("slot{slot}.bus"));
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
                slot: *slot,
                version: *version,
                flags,
                image_id,
                rom_len: rom.len() as u32,
                ram_len,
                memory_path: memory_path.map(|p| p.display().to_string()).unwrap_or_default(),
                bus_path: if config.shm { bus_path.display().to_string() } else { String::new() },
                session_dir: config.session.display().to_string(),
                rom: rom.to_vec(),
            };
            let mut out = Vec::new();
            hello.encode(&mut out);
            let mut sock_end = SocketEnd::new(stream)?;
            stream_timeout(&sock_end.stream, Some(Duration::from_secs(10)));
            let mut reply = Vec::new();
            let handshake = sock_end.send(&out).and_then(|_| sock_end.recv(&mut reply)?.ok_or_else(|| "closed before its HELLO reply".to_string()));
            if let Err(e) = handshake {
                reap(&mut driver_child);
                return Err(format!("slot {slot} ({program}): {e}"));
            }
            stream_timeout(&sock_end.stream, None);
            let (rslot, transport, info) = match DeviceInfo::decode(&reply) {
                Ok(v) => v,
                Err(e) => {
                    reap(&mut driver_child);
                    return Err(format!("slot {slot} ({program}): {e}"));
                }
            };
            if rslot != *slot || info.version != *version || (transport == proto::TRANSPORT_SHM && !config.shm) || transport > 1 {
                reap(&mut driver_child);
                return Err(format!("slot {slot} ({program}): it answered for slot {rslot}, version {}, transport {transport}", info.version));
            }
            let link = if transport == proto::TRANSPORT_SHM {
                let m = Mapping::open(&bus_path, true)?;
                Link::Shm(ShmEnd::new(m, true, config.spin, sock_end.stream)?)
            } else {
                Link::Socket(sock_end)
            };
            let mut table = vec![None; 256];
            for e in &info.table {
                table[e.function as usize] = Some(*e);
            }
            bus.by_slot[*slot as usize] = Some(bus.drivers.len());
            bus.drivers.push(Driver {
                slot: *slot,
                info,
                transport,
                table,
                link,
                child: driver_child,
                batch: Vec::new(),
                batch_count: 0,
                held: Vec::new(),
                out: Vec::new(),
                reply: Vec::new(),
                stats: DriverStats::default(),
                command: command.join(" "),
            });
        }
        Ok(bus)
    }

    pub fn driver(&mut self, slot: u8) -> Option<&mut Driver> {
        self.by_slot[slot as usize].map(|i| &mut self.drivers[i])
    }

    /// The layers of every driver: (z, slot, declaration order, layer).
    pub fn layers(&self) -> Vec<(u8, u8, usize, LayerDecl)> {
        let mut v: Vec<_> = self.drivers.iter().flat_map(|d| d.info.layers.iter().enumerate().map(|(i, l)| (l.z, d.slot, i, *l))).collect();
        v.sort_by_key(|&(z, s, i, _)| (z, s, i));
        v
    }

    /// The first native size a driver proposed.
    pub fn native(&self) -> Option<(u32, u32)> {
        self.drivers.iter().map(|d| d.info.native).find(|&(w, h)| w > 0 && h > 0).map(|(w, h)| (w as u32, h as u32))
    }

    /// Asks every driver to reset.
    pub fn reset(&mut self) -> Result<(), String> {
        for d in &mut self.drivers {
            d.request(msg::RESET, |_| {}, None)?;
        }
        Ok(())
    }

    /// Restores the slot's state section (RESTORE).
    pub fn restore(&mut self, slot: u8, bytes: &[u8]) -> Result<bool, String> {
        let d = self.driver(slot).ok_or("no such driver")?;
        let reply = d.request(msg::RESTORE, |w| {
            w.u8(0).u16(0).blob(bytes);
        }, None)?;
        Ok(reply.get(1) == Some(&1))
    }

    /// Ends the session: SHUTDOWN to each driver, then waits for it (and
    /// kills one that does not exit).
    pub fn shutdown(&mut self) {
        for d in &mut self.drivers {
            d.out.clear();
            d.take_batch();
            Writer::new(&mut d.out, msg::SHUTDOWN).finish();
            let out = std::mem::take(&mut d.out);
            let _ = d.link.send(&out);
            if let Link::Socket(s) = &d.link {
                let _ = s.stream.shutdown(std::net::Shutdown::Both);
            }
            if let Link::Shm(s) = &d.link {
                let _ = s.socket.shutdown(std::net::Shutdown::Both);
            }
        }
        for d in &mut self.drivers {
            reap(&mut d.child);
        }
    }

    /// PRESENT to each visual driver in slot order (DEVICES.md 7). Returns
    /// per layer (slot, id): a new file's path, and the damage.
    pub fn present(&mut self, picture: (u32, u32), screen: (f32, f32, f32, f32)) -> Result<Vec<LayerUpdate>, String> {
        let mut props: Vec<(u8, u8, u32)> = Vec::new();
        let mut updates = Vec::new();
        for d in &mut self.drivers {
            let slot = d.slot;
            let given = props.clone();
            let reply = d.request(
                msg::PRESENT,
                |w| {
                    w.u8(0).u16(0).u16(picture.0 as u16).u16(picture.1 as u16);
                    w.f32(screen.0).f32(screen.1).f32(screen.2).f32(screen.3).u32(given.len() as u32);
                    for &(s, k, v) in &given {
                        w.u8(s).u8(k).u16(0).u32(v);
                    }
                },
                None,
            )?;
            let mut r = Reader::new(&reply);
            r.u8()?;
            let k = r.u8()?;
            r.skip(2)?;
            for _ in 0..k {
                let id = r.u8()?;
                let flags = r.u8()?;
                let path = if flags & 1 != 0 { Some(PathBuf::from(String::from_utf8_lossy(r.str()?).into_owned())) } else { None };
                let n = r.u32()?;
                let mut damage = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    damage.push([r.i32()?, r.i32()?, r.i32()?, r.i32()?]);
                }
                updates.push(LayerUpdate { slot, id, path, damage });
            }
            let p = r.u32()?;
            for _ in 0..p {
                let key = r.u8()?;
                r.skip(3)?;
                props.push((slot, key, r.u32()?));
            }
        }
        Ok(updates)
    }

    /// Sends what is batched for every driver (before something that
    /// needs it).
    pub fn flush(&mut self) -> Result<(), String> {
        for d in &mut self.drivers {
            // On shared memory the batch goes with the next request.
            if d.batch_count > 0 && matches!(d.link, Link::Socket(_)) {
                d.out.clear();
                d.take_batch();
                let out = std::mem::take(&mut d.out);
                d.link.send(&out)?;
                d.out = out;
            }
        }
        Ok(())
    }
}

/// A layer's news from PRESENT.
pub struct LayerUpdate {
    pub slot: u8,
    pub id: u8,
    pub path: Option<PathBuf>,
    pub damage: Vec<[i32; 4]>,
}

fn stream_timeout(s: &UnixStream, t: Option<Duration>) {
    let _ = s.set_read_timeout(t);
    let _ = s.set_write_timeout(t);
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
            return Err("did not connect".into());
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

impl Driver {
    /// Puts the batch (if any) in front of `out`.
    fn take_batch(&mut self) {
        // Messages with no reply held for shared memory go first.
        self.out.append(&mut self.held);
        if self.batch_count == 0 {
            return;
        }
        let mut w = Writer::new(&mut self.out, msg::BATCH);
        w.u8(0).u16(0).u32(self.batch_count).bytes(&self.batch);
        w.finish();
        self.stats.batches += 1;
        self.batch.clear();
        self.batch_count = 0;
    }

    /// Sends a request of type `kind` (body written by `f` after the type
    /// byte), with the batch in front, and waits for its reply, serving
    /// READs from `mem`. Returns the reply's body.
    fn request(&mut self, kind: u8, f: impl FnOnce(&mut Writer), mut mem: Option<&mut GuestMem>) -> Result<Vec<u8>, String> {
        self.out.clear();
        self.take_batch();
        let mut w = Writer::new(&mut self.out, kind);
        f(&mut w);
        w.finish();
        let out = std::mem::take(&mut self.out);
        self.link.send(&out)?;
        self.out = out;
        let mut reply = std::mem::take(&mut self.reply);
        loop {
            self.link.recv(&mut reply)?;
            match reply.first() {
                Some(&t) if t == kind | msg::REPLY => break,
                Some(&msg::READ) => {
                    let mut r = Reader::new(&reply[1..]);
                    r.skip(3)?;
                    let (addr, len) = (r.u32()?, r.u32()? as usize);
                    self.out.clear();
                    let mut w = Writer::new(&mut self.out, msg::DATA);
                    let data = mem.as_mut().and_then(|m| readable(m.rom, m.ram, addr, len));
                    match data {
                        Some(b) => w.u8(1).u16(0).bytes(b),
                        None => w.u8(0).u16(0),
                    };
                    w.finish();
                    let out = std::mem::take(&mut self.out);
                    self.link.send(&out)?;
                    self.out = out;
                }
                Some(&msg::ERROR) => return Err(format!("slot {}: {}", self.slot, String::from_utf8_lossy(&reply[1..]))),
                other => return Err(format!("slot {}: unexpected message {:02x?} for {kind:02x}", self.slot, other)),
            }
        }
        let body = reply.clone();
        self.reply = reply;
        Ok(body)
    }
}

/// Copies writes (address, length, bytes) into RAM, each within `[lo, hi)`.
fn apply_writes(r: &mut Reader, ram: &mut [u8], within: Option<(u32, u32)>, slot: u8) -> Result<(), String> {
    let w = r.u32()?;
    for _ in 0..w {
        let addr = r.u32()?;
        let bytes = r.blob()?;
        let ok = match within {
            Some((lo, hi)) => addr >= lo && (addr as u64 + bytes.len() as u64) <= hi as u64,
            None => true,
        };
        let at = ram_range(ram.len(), addr, bytes.len()).filter(|_| ok).ok_or(format!("slot {slot}: a write outside its buffer"))?;
        ram[at..at + bytes.len()].copy_from_slice(bytes);
    }
    Ok(())
}

impl Devices for Bus {
    fn begin_frame(&mut self, frame: u32, time: u32, host: &[Event], replay: Option<&[(u8, Vec<u8>)]>, mut mem: GuestMem) -> Result<FrameInputs, String> {
        let mut inputs = FrameInputs::default();
        // Send BEGIN to all, then collect: the drivers work at once.
        let mut msg_out = Vec::new();
        for d in &mut self.drivers {
            d.out.clear();
            d.take_batch();
            let mut w = Writer::new(&mut d.out, msg::BEGIN);
            let record = replay.map(|r| r.iter().find(|(s, _)| *s == d.slot).map_or(&[][..], |(_, b)| &b[..]));
            w.u8(record.is_some() as u8).u16(0).u32(frame).u32(time).u32(host.len() as u32);
            for e in host {
                w.bytes(&[e.kind, e.a, e.b, e.c]);
            }
            w.blob(record.unwrap_or(&[]));
            w.finish();
            std::mem::swap(&mut d.out, &mut msg_out);
            d.link.send(&msg_out)?;
            std::mem::swap(&mut d.out, &mut msg_out);
        }
        for d in &mut self.drivers {
            let start = Instant::now();
            let mut reply = std::mem::take(&mut self.scratch);
            loop {
                d.link.recv(&mut reply)?;
                match reply.first() {
                    Some(&t) if t == msg::BEGIN | msg::REPLY => break,
                    Some(&msg::READ) => {
                        let mut r = Reader::new(&reply[1..]);
                        r.skip(3)?;
                        let (addr, len) = (r.u32()?, r.u32()? as usize);
                        d.out.clear();
                        let mut w = Writer::new(&mut d.out, msg::DATA);
                        match readable(mem.rom, mem.ram, addr, len) {
                            Some(b) => w.u8(1).u16(0).bytes(b),
                            None => w.u8(0).u16(0),
                        };
                        w.finish();
                        let out = std::mem::take(&mut d.out);
                        d.link.send(&out)?;
                        d.out = out;
                    }
                    Some(&msg::ERROR) => return Err(format!("slot {}: {}", d.slot, String::from_utf8_lossy(&reply[1..]))),
                    other => return Err(format!("slot {}: unexpected message {other:02x?} for BEGIN", d.slot)),
                }
            }
            d.stats.begin.add(start.elapsed());
            let mut r = Reader::new(&reply);
            r.skip(4)?;
            inputs.charge += r.u32()?;
            let e = r.u32()?;
            for _ in 0..e {
                let b = r.take(4)?;
                let event = Event { kind: b[0], a: b[1], b: b[2], c: b[3] };
                inputs.events.push(PosEvent { event, position: r.u8()?, flags: r.u8()?, index: r.u16()? });
            }
            apply_writes(&mut r, mem.ram, None, d.slot)?;
            let record = r.blob()?;
            if !record.is_empty() {
                inputs.records.push((d.slot, record.to_vec()));
            }
            self.scratch = reply;
        }
        // Drivers' events in slot order (they are: drivers are in slot
        // order), each driver's in its order.
        let _ = &mut mem;
        Ok(inputs)
    }

    fn call(&mut self, slot: u8, function: u8, args: [u32; 6], budget: i64, mem: GuestMem) -> Result<CallOutcome, String> {
        let batching = self.batching;
        let Some(d) = self.driver(slot) else { return Ok(CallOutcome { a0: ERR_UNKNOWN, a1: None, charge: 0 }) };
        let Some(e) = d.table[function as usize] else { return Ok(CallOutcome { a0: ERR_UNKNOWN, a1: None, charge: 0 }) };
        // DEVICES.md 5: the core's checks.
        let mut buf: (u32, u32) = (0, 0);
        if e.has_buffer() {
            let Some(len) = e.buffer_len(&args) else {
                d.stats.checked_out += 1;
                return Ok(CallOutcome { a0: e.len_bad, a1: None, charge: 0 });
            };
            let addr = args[e.buf_addr_arg as usize];
            let rom_ok = e.access & access::ROM != 0 && e.access & access::WRITE == 0;
            let placed = len == 0
                || ram_range(mem.ram.len(), addr, len as usize).is_some()
                || (rom_ok && readable(mem.rom, &[], addr, len as usize).is_some());
            if !placed {
                d.stats.checked_out += 1;
                return Ok(CallOutcome { a0: e.place_bad, a1: None, charge: 0 });
            }
            buf = (addr, len);
        }
        if e.check_arg != proto::NO_ARG && args[e.check_arg as usize] > e.check_max {
            d.stats.checked_out += 1;
            return Ok(CallOutcome { a0: e.place_bad, a1: None, charge: 0 });
        }
        let bytes: &[u8] = if e.has_buffer() && e.access & access::READ != 0 && buf.1 > 0 {
            readable(mem.rom, mem.ram, buf.0, buf.1 as usize).unwrap_or(&[])
        } else {
            &[]
        };
        if batching && e.batchable() {
            let (a0, a1, charge) = e.batch_outcome(&args, buf.1);
            proto::encode_call(&mut d.batch, function, &args, budget, bytes);
            d.batch_count += 1;
            d.stats.batched += 1;
            return Ok(CallOutcome { a0, a1, charge });
        }
        // A real round trip. On the socket the read buffer goes inline (a
        // budget-bounded one only as far as the budget reaches).
        let inline = match d.link {
            Link::Socket(_) if e.access & access::BUDGET_BOUNDED != 0 => &bytes[..bytes.len().min(budget.max(1) as usize)],
            Link::Socket(_) => bytes,
            Link::Shm(_) => &[],
        };
        let start = Instant::now();
        d.out.clear();
        d.take_batch();
        proto::encode_call(&mut d.out, function, &args, budget, inline);
        let out = std::mem::take(&mut d.out);
        d.link.send(&out)?;
        d.out = out;
        let mut reply = std::mem::take(&mut d.reply);
        let mut gm = mem;
        loop {
            d.link.recv(&mut reply)?;
            match reply.first() {
                Some(&msg::RESULT) => break,
                Some(&msg::READ) => {
                    let mut r = Reader::new(&reply[1..]);
                    r.skip(3)?;
                    let (addr, len) = (r.u32()?, r.u32()? as usize);
                    d.out.clear();
                    let mut w = Writer::new(&mut d.out, msg::DATA);
                    match readable(gm.rom, gm.ram, addr, len) {
                        Some(b) => w.u8(1).u16(0).bytes(b),
                        None => w.u8(0).u16(0),
                    };
                    w.finish();
                    let out = std::mem::take(&mut d.out);
                    d.link.send(&out)?;
                    d.out = out;
                }
                Some(&msg::ERROR) => return Err(format!("slot {slot}: {}", String::from_utf8_lossy(&reply[1..]))),
                other => return Err(format!("slot {slot}: unexpected message {other:02x?} for a call")),
            }
        }
        let mut r = Reader::new(&reply);
        r.skip(1)?;
        let flags = r.u8()?;
        r.skip(2)?;
        let (a0, a1, charge) = (r.u32()?, r.u32()?, r.u32()?);
        let within = (e.has_buffer() && e.access & access::WRITE != 0).then_some((buf.0, buf.0.wrapping_add(buf.1)));
        apply_writes(&mut r, gm.ram, Some(within.unwrap_or((0, 0))), slot)?;
        d.reply = reply;
        d.stats.calls.add(start.elapsed());
        let _ = &mut gm;
        Ok(CallOutcome { a0, a1: (flags & 1 != 0).then_some(a1), charge })
    }

    fn end_frame(&mut self, frame: u32, stopped: bool, _mem: GuestMem) -> Result<(), String> {
        for d in &mut self.drivers {
            d.out.clear();
            d.take_batch();
            let mut w = Writer::new(&mut d.out, msg::END);
            w.u8(stopped as u8).u16(0).u32(frame);
            w.finish();
            match d.link {
                // One packet at a time in the request area: a message with
                // no reply waits for the next request (DEVICES.md 4.2).
                Link::Shm(_) => d.held.append(&mut d.out),
                Link::Socket(_) => {
                    let out = std::mem::take(&mut d.out);
                    d.link.send(&out)?;
                    d.out = out;
                }
            }
        }
        Ok(())
    }

    fn state(&mut self, slot: u8) -> Result<Vec<u8>, String> {
        let d = self.driver(slot).ok_or(format!("no driver for slot {slot}"))?;
        let reply = d.request(msg::STATE, |_| {}, None)?;
        let mut r = Reader::new(&reply);
        r.skip(4)?;
        Ok(r.blob()?.to_vec())
    }
}
