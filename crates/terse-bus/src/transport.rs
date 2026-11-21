//! The two transports (DEVICES.md 3, 4): a Unix-domain socket carrying
//! framed messages, and shared memory: a bus file with a request and a
//! reply area, handed over with sequence words, an end that waits
//! sleeping on the system's address wait (Linux futexes, macOS ulocks) or
//! on the socket ([`crate::sys`], DEVICES.md 4.3).

use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::proto::MAX_MESSAGE;
use crate::sys;
pub use crate::sys::{UnixListener, UnixStream, peer_gone};

/// A file mapped into memory, shared with other processes.
pub struct Mapping {
    ptr: *mut u8,
    len: usize,
    _file: File,
}

// The mapping is plain memory; who writes it when is the protocol's rule.
unsafe impl Send for Mapping {}

impl Mapping {
    /// Creates `path` (`len` bytes, zero) and maps it.
    pub fn create(path: &Path, len: usize) -> Result<Self, String> {
        let file = OpenOptions::new().read(true).write(true).create_new(true).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        file.set_len(len as u64).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::map(file, len, true)
    }

    /// Maps an existing file, for reading and writing or read-only.
    pub fn open(path: &Path, writable: bool) -> Result<Self, String> {
        let file = OpenOptions::new().read(true).write(writable).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let len = file.metadata().map_err(|e| e.to_string())?.len() as usize;
        Self::map(file, len, writable)
    }

    fn map(file: File, len: usize, writable: bool) -> Result<Self, String> {
        if len == 0 {
            return Err("cannot map an empty file".into());
        }
        let ptr = sys::map(&file, len, writable)?;
        Ok(Mapping { ptr, len, _file: file })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr
    }

    /// The bytes. Other processes may change them: only read them while
    /// the protocol says they are still.
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: the mapping lives as long as self.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    #[allow(clippy::mut_from_ref)]
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: as above; writable mappings only (the caller's promise).
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }

    fn word(&self, at: usize) -> &AtomicU32 {
        assert!(at + 4 <= self.len && at % 4 == 0);
        // SAFETY: aligned, in the mapping (page aligned base).
        unsafe { &*(self.ptr.add(at) as *const AtomicU32) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: our own mapping.
        unsafe {
            sys::unmap(self.ptr, self.len);
        }
    }
}

// The bus file's control block (DEVICES.md 4.2).
const MAGIC: u32 = u32::from_le_bytes(*b"TBUS");
const CONTROL: usize = 4096;
pub const AREA: usize = 4 << 20;
const REQ_SEQ: usize = 64;
const REQ_LEN: usize = 68;
const DRIVER_SLEEPING: usize = 72;
const REP_SEQ: usize = 128;
const REP_LEN: usize = 132;
const CORE_SLEEPING: usize = 136;

/// A sleeping word's values (DEVICES.md 4.2): awake (spinning or
/// working); asleep on the system's address wait on the sequence word
/// (woken by the system's address wake); asleep on the socket (woken by a
/// byte written to it).
pub const AWAKE: u32 = 0;
pub const SLEEPS_ON_ADDRESS: u32 = 1;
pub const SLEEPS_ON_SOCKET: u32 = 2;

/// The longest sleep before an end looks again (and checks that the other
/// end is alive): a wake that is lost costs at most this.
const NAP: Duration = Duration::from_millis(50);

/// Whether an end sleeps on the socket: where the system has no address
/// wait across processes (Windows), or when `TERSE_BUS_WAKE=socket` asks
/// (to test that way on any system).
pub fn wake_by_socket() -> bool {
    !sys::address_wait_available() || std::env::var("TERSE_BUS_WAKE").is_ok_and(|v| v == "socket")
}

/// The bus file's size.
pub const BUS_FILE: usize = CONTROL + 2 * AREA;

/// Initialises a new bus file's control block.
pub fn init_bus(m: &Mapping) {
    m.word(0).store(MAGIC, Ordering::Relaxed);
    m.word(4).store(0, Ordering::Relaxed);
    m.word(8).store(CONTROL as u32, Ordering::Relaxed);
    m.word(12).store(AREA as u32, Ordering::Relaxed);
    m.word(16).store((CONTROL + AREA) as u32, Ordering::Relaxed);
    m.word(20).store(AREA as u32, Ordering::Relaxed);
}

/// How long to spin before sleeping (DEVICES.md 4.2): at most `max`; when
/// adaptive, only about twice as long as the longest of the recent waits
/// that ended while spinning, so that an end idle between frames sleeps
/// almost at once while the calls of a busy frame still need no system
/// call.
#[derive(Clone, Copy, Debug)]
pub struct Spin {
    pub max: Duration,
    pub adaptive: bool,
}

impl Spin {
    pub fn fixed(max: Duration) -> Self {
        Spin { max, adaptive: false }
    }
}

/// The shortest adaptive spin.
const MIN_SPIN: Duration = Duration::from_micros(4);
/// Waits remembered for the adaptive spin.
const RECENT: usize = 32;

/// One side of a shared-memory bus: `core` writes requests and reads
/// replies; the driver the other way round.
pub struct ShmEnd {
    map: Mapping,
    core: bool,
    /// The last sequence number seen from the other side.
    seen: u32,
    pub spin: Spin,
    /// Recent waits (µs; `u32::MAX` for one that slept).
    recent: [u32; RECENT],
    next: usize,
    /// Giving up on the other end after this long (a hung driver).
    pub deadline: Option<Duration>,
    /// For noticing that the other end has gone, and for waking an end
    /// that sleeps on it.
    pub socket: UnixStream,
    /// This end sleeps on the socket rather than on the address.
    pub sleep_on_socket: bool,
    /// Time spent spinning, and waits that slept (measurements).
    pub spun: Duration,
    pub sleeps: u64,
}

impl ShmEnd {
    pub fn new(map: Mapping, core: bool, spin: Spin, socket: UnixStream) -> Result<Self, String> {
        if map.len() < BUS_FILE || map.word(0).load(Ordering::Relaxed) != MAGIC {
            return Err("not a bus file".into());
        }
        let seen = map.word(if core { REP_SEQ } else { REQ_SEQ }).load(Ordering::Acquire);
        Ok(ShmEnd {
            map,
            core,
            seen,
            spin,
            recent: [0; RECENT],
            next: 0,
            deadline: None,
            socket,
            sleep_on_socket: wake_by_socket(),
            spun: Duration::ZERO,
            sleeps: 0,
        })
    }

    fn out_area(&self) -> usize {
        if self.core { CONTROL } else { CONTROL + AREA }
    }

    fn in_area(&self) -> usize {
        if self.core { CONTROL + AREA } else { CONTROL }
    }

    /// How long to spin now.
    fn spin_limit(&self) -> Duration {
        if !self.spin.adaptive {
            return self.spin.max;
        }
        let max = self.spin.max.as_micros().min(u32::MAX as u128 - 1) as u32;
        match self.recent.iter().copied().filter(|&w| w < max).max() {
            Some(w) => Duration::from_micros((2 * w as u64).max(MIN_SPIN.as_micros() as u64)).min(self.spin.max),
            None => MIN_SPIN.min(self.spin.max),
        }
    }

    /// Writes a packet and hands it over, waking the other end the way it
    /// sleeps.
    pub fn send(&mut self, packet: &[u8]) -> Result<(), String> {
        if packet.len() > AREA {
            return Err(format!("a packet of {} bytes is larger than the bus area", packet.len()));
        }
        let at = self.out_area();
        let (seq, len, sleeping) = if self.core { (REQ_SEQ, REQ_LEN, DRIVER_SLEEPING) } else { (REP_SEQ, REP_LEN, CORE_SLEEPING) };
        self.map.bytes_mut()[at..at + packet.len()].copy_from_slice(packet);
        self.map.word(len).store(packet.len() as u32, Ordering::Relaxed);
        // Sequentially consistent, so that the read of the other side's
        // sleeping word cannot move before it (on ARM a release alone
        // would let it: a wake missed, a nap taken).
        self.map.word(seq).fetch_add(1, Ordering::SeqCst);
        match self.map.word(sleeping).load(Ordering::SeqCst) {
            AWAKE => {}
            SLEEPS_ON_SOCKET => {
                // One byte, of any value: the other end drops it. A failed
                // write means the other end has gone, which its next wait
                // finds out.
                let _ = self.socket.write_all(&[0]);
            }
            _ => sys::address_wake(self.map.word(seq)),
        }
        Ok(())
    }

    /// Waits for the other side's next packet and returns it (valid until
    /// the next send). An error: the other end has gone, or (with a
    /// deadline) has not answered in time.
    pub fn recv(&mut self) -> Result<&[u8], String> {
        let (seq, len, sleeping) = if self.core { (REP_SEQ, REP_LEN, CORE_SLEEPING) } else { (REQ_SEQ, REQ_LEN, DRIVER_SLEEPING) };
        let start = Instant::now();
        let limit = self.spin_limit();
        let mut spins = 0u32;
        let mut slept = false;
        loop {
            let now = self.map.word(seq).load(Ordering::Acquire);
            if now != self.seen {
                self.seen = now;
                break;
            }
            spins += 1;
            if !slept && !limit.is_zero() && (spins & 63 != 0 || start.elapsed() < limit) {
                std::hint::spin_loop();
                continue;
            }
            if !slept {
                self.spun += start.elapsed();
                self.sleeps += 1;
                slept = true;
            }
            // Sleep until the number changes: say how, look again, sleep.
            let how = if self.sleep_on_socket { SLEEPS_ON_SOCKET } else { SLEEPS_ON_ADDRESS };
            self.map.word(sleeping).store(how, Ordering::SeqCst);
            let mut gone = false;
            if self.map.word(seq).load(Ordering::SeqCst) == self.seen {
                if self.sleep_on_socket {
                    // Woken by a byte, or by the end of the stream.
                    gone = sys::wait_readable(&self.socket, NAP) && !sys::drain(&self.socket);
                } else {
                    sys::address_wait(self.map.word(seq), self.seen, NAP);
                }
            }
            self.map.word(sleeping).store(AWAKE, Ordering::SeqCst);
            if self.map.word(seq).load(Ordering::Acquire) == self.seen {
                if gone || peer_gone(&self.socket) {
                    return Err("the other end has gone".into());
                }
                if let Some(d) = self.deadline.filter(|&d| start.elapsed() > d) {
                    return Err(format!("no reply within {:.1} s", d.as_secs_f64()));
                }
            }
        }
        let waited = start.elapsed();
        if !slept {
            self.spun += waited;
        }
        self.recent[self.next] = if slept { u32::MAX } else { waited.as_micros().min(u32::MAX as u128 - 1) as u32 };
        self.next = (self.next + 1) % RECENT;
        let n = self.map.word(len).load(Ordering::Relaxed) as usize;
        let at = self.in_area();
        Ok(&self.map.bytes()[at..at + n.min(AREA)])
    }
}

/// A socket end: framed messages on a Unix-domain stream.
pub struct SocketEnd {
    pub stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl SocketEnd {
    pub fn new(stream: UnixStream) -> Result<Self, String> {
        let reader = BufReader::with_capacity(256 << 10, stream.try_clone().map_err(|e| e.to_string())?);
        Ok(SocketEnd { stream, reader })
    }

    pub fn send(&mut self, packet: &[u8]) -> Result<(), String> {
        self.stream.write_all(packet).map_err(|e| format!("send: {e}"))
    }

    /// Gives up waiting for a message after `d` (a hung driver), or never.
    pub fn set_deadline(&self, d: Option<Duration>) {
        let _ = self.stream.set_read_timeout(d);
    }

    /// The next message (what follows its length: address and body) into
    /// `buf`. `None` at the end of the stream.
    pub fn recv(&mut self, buf: &mut Vec<u8>) -> Result<Option<()>, String> {
        let mut head = [0u8; 4];
        match self.reader.read_exact(&mut head) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                return Err("no reply in time".into());
            }
            Err(e) => return Err(format!("receive: {e}")),
        }
        let n = u32::from_le_bytes(head) as usize;
        if n > MAX_MESSAGE {
            return Err(format!("a message of {n} bytes"));
        }
        buf.clear();
        buf.resize(n, 0);
        self.reader.read_exact(buf).map_err(|e| format!("receive: {e}"))?;
        Ok(Some(()))
    }
}

/// Pins the calling thread to `cpu` (where the system can: not macOS).
pub fn pin_to(cpu: usize) {
    sys::pin_to(cpu);
}

/// The longest path a socket may have on every system (`sun_path` is 104
/// bytes on macOS, 108 on Linux and Windows, with its NUL).
pub const SOCKET_PATH_MAX: usize = 103;

/// A directory, only the user's, for the session's shared files and
/// sockets (DEVICES.md 4.3): in `/dev/shm` on Linux, `/tmp` on macOS, the
/// user's temporary directory on Windows, or `TERSE_BUS_DIR`.
pub fn session_dir() -> Result<std::path::PathBuf, String> {
    let base = sys::session_base();
    let mut seed = [0u8; 8];
    getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
    let dir = base.join(format!("terse-bus-{}-{:016x}", std::process::id(), u64::from_le_bytes(seed)));
    // The longest socket in it.
    let longest = dir.join("driver99.sock");
    if longest.to_str().is_none_or(|p| p.len() > SOCKET_PATH_MAX) {
        return Err(format!("{}: too long (or not UTF-8) for a socket's path; set TERSE_BUS_DIR to a short directory", dir.display()));
    }
    sys::make_private_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Packets go both ways through a bus file, sleeping and spinning,
    /// each end sleeping on the address (where the system has one) or on
    /// the socket, in every combination.
    #[test]
    fn shared_memory_hand_over() {
        for (core_socket, driver_socket) in [(false, false), (true, true), (false, true), (true, false)] {
            if !sys::address_wait_available() && !(core_socket && driver_socket) {
                continue;
            }
            let dir = session_dir().unwrap();
            let path = dir.join("bus");
            let m = Mapping::create(&path, BUS_FILE).unwrap();
            init_bus(&m);
            let (a, b) = sys::pair().unwrap();
            let mut core = ShmEnd::new(m, true, Spin::fixed(Duration::from_micros(50)), a).unwrap();
            core.sleep_on_socket = core_socket;
            // The driver maps the file before the core's first packet, as
            // after the handshake.
            let mut d = ShmEnd::new(Mapping::open(&path, true).unwrap(), false, Spin { max: Duration::from_micros(200), adaptive: true }, b).unwrap();
            d.sleep_on_socket = driver_socket;
            let driver = std::thread::spawn(move || {
                for i in 0..1000 {
                    // Now and then the core is slow, so that this end sleeps.
                    let p = d.recv().unwrap().to_vec();
                    let reply: Vec<u8> = p.iter().map(|x| x.wrapping_add(1)).collect();
                    if i % 100 == 0 {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    d.send(&reply).unwrap();
                }
                // The core goes: the next wait says so.
                assert!(d.recv().is_err());
            });
            let mut napped = 0;
            for i in 0..1000u32 {
                if i % 100 == 50 {
                    std::thread::sleep(Duration::from_millis(2));
                }
                let p = i.to_le_bytes();
                let start = Instant::now();
                core.send(&p).unwrap();
                let r = core.recv().unwrap();
                assert_eq!(r, p.map(|x| x.wrapping_add(1)));
                if start.elapsed() >= NAP - Duration::from_millis(5) {
                    napped += 1;
                }
            }
            // The sleeps (about 20) were woken: a lost wake would have cost
            // a whole nap. A few slow round trips are a busy computer.
            assert!(napped < 5, "core {core_socket}, driver {driver_socket}: {napped} round trips took a nap");
            drop(core.socket.shutdown(std::net::Shutdown::Both));
            drop(core);
            driver.join().unwrap();
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
