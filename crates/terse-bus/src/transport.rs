//! The two transports (DEVICES.md 3, 4): a Unix socket carrying framed
//! messages, and shared memory: a bus file with a request and a reply
//! area, handed over with sequence words and Linux futexes.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::proto::MAX_MESSAGE;

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
        let prot = if writable { libc::PROT_READ | libc::PROT_WRITE } else { libc::PROT_READ };
        // SAFETY: a fresh shared mapping of a file we hold open.
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, prot, libc::MAP_SHARED, file.as_raw_fd(), 0) };
        if ptr == libc::MAP_FAILED {
            return Err(format!("mmap: {}", std::io::Error::last_os_error()));
        }
        Ok(Mapping { ptr: ptr as *mut u8, len, _file: file })
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
            libc::munmap(self.ptr as *mut libc::c_void, self.len);
        }
    }
}

fn futex_wait(word: &AtomicU32, expected: u32, timeout: Duration) {
    let ts = libc::timespec { tv_sec: timeout.as_secs() as libc::time_t, tv_nsec: timeout.subsec_nanos() as libc::c_long };
    // SAFETY: a futex on a word of a shared mapping (not private: shared
    // between processes).
    unsafe {
        libc::syscall(libc::SYS_futex, word.as_ptr(), libc::FUTEX_WAIT, expected, &ts as *const libc::timespec, 0usize, 0u32);
    }
}

fn futex_wake(word: &AtomicU32) {
    // SAFETY: as above.
    unsafe {
        libc::syscall(libc::SYS_futex, word.as_ptr(), libc::FUTEX_WAKE, i32::MAX, 0usize, 0usize, 0u32);
    }
}

/// Whether the peer of a socket has closed it (without reading anything).
pub fn peer_gone(sock: &UnixStream) -> bool {
    let mut p = libc::pollfd { fd: sock.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    // SAFETY: one pollfd, no wait.
    let n = unsafe { libc::poll(&mut p, 1, 0) };
    if n <= 0 {
        return false;
    }
    if p.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
        return true;
    }
    // Readable: EOF if a peek reads nothing.
    let mut b = [0u8; 1];
    // SAFETY: peek one byte.
    let r = unsafe { libc::recv(sock.as_raw_fd(), b.as_mut_ptr() as *mut libc::c_void, 1, libc::MSG_PEEK | libc::MSG_DONTWAIT) };
    r == 0
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

/// One side of a shared-memory bus: `core` writes requests and reads
/// replies; the driver the other way round.
pub struct ShmEnd {
    map: Mapping,
    core: bool,
    /// The last sequence number seen from the other side.
    seen: u32,
    /// How long to spin before sleeping.
    pub spin: Duration,
    /// For noticing that the other end has gone.
    pub socket: UnixStream,
}

impl ShmEnd {
    pub fn new(map: Mapping, core: bool, spin: Duration, socket: UnixStream) -> Result<Self, String> {
        if map.len() < BUS_FILE || map.word(0).load(Ordering::Relaxed) != MAGIC {
            return Err("not a bus file".into());
        }
        let seen = map.word(if core { REP_SEQ } else { REQ_SEQ }).load(Ordering::Acquire);
        Ok(ShmEnd { map, core, seen, spin, socket })
    }


    fn out_area(&self) -> usize {
        if self.core { CONTROL } else { CONTROL + AREA }
    }

    fn in_area(&self) -> usize {
        if self.core { CONTROL + AREA } else { CONTROL }
    }

    /// Writes a packet and hands it over.
    pub fn send(&mut self, packet: &[u8]) -> Result<(), String> {
        if packet.len() > AREA {
            return Err(format!("a packet of {} bytes is larger than the bus area", packet.len()));
        }
        let at = self.out_area();
        let (seq, len, sleeping) = if self.core { (REQ_SEQ, REQ_LEN, DRIVER_SLEEPING) } else { (REP_SEQ, REP_LEN, CORE_SLEEPING) };
        self.map.bytes_mut()[at..at + packet.len()].copy_from_slice(packet);
        self.map.word(len).store(packet.len() as u32, Ordering::Relaxed);
        self.map.word(seq).fetch_add(1, Ordering::Release);
        if self.map.word(sleeping).load(Ordering::SeqCst) != 0 {
            futex_wake(self.map.word(seq));
        }
        Ok(())
    }

    /// Waits for the other side's next packet and returns it (valid until
    /// the next send). `None`: the other end has gone.
    pub fn recv(&mut self) -> Option<&[u8]> {
        let (seq, len, sleeping) = if self.core { (REP_SEQ, REP_LEN, CORE_SLEEPING) } else { (REQ_SEQ, REQ_LEN, DRIVER_SLEEPING) };
        let start = Instant::now();
        let mut spins = 0u32;
        loop {
            let now = self.map.word(seq).load(Ordering::Acquire);
            if now != self.seen {
                self.seen = now;
                break;
            }
            spins += 1;
            if !self.spin.is_zero() && (spins & 63 != 0 || start.elapsed() < self.spin) {
                std::hint::spin_loop();
                continue;
            }
            // Sleep until the number changes.
            self.map.word(sleeping).store(1, Ordering::SeqCst);
            if self.map.word(seq).load(Ordering::SeqCst) == self.seen {
                futex_wait(self.map.word(seq), self.seen, Duration::from_millis(100));
            }
            self.map.word(sleeping).store(0, Ordering::SeqCst);
            if self.map.word(seq).load(Ordering::Acquire) == self.seen && peer_gone(&self.socket) {
                return None;
            }
        }
        let n = self.map.word(len).load(Ordering::Relaxed) as usize;
        let at = self.in_area();
        Some(&self.map.bytes()[at..at + n.min(AREA)])
    }
}

/// A socket end: framed messages on a Unix stream.
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

    /// The next message's body into `buf`. `None` at the end of the stream.
    pub fn recv(&mut self, buf: &mut Vec<u8>) -> Result<Option<()>, String> {
        let mut head = [0u8; 4];
        match self.reader.read_exact(&mut head) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
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

/// Pins the calling thread to `cpu`.
pub fn pin_to(cpu: usize) {
    // SAFETY: a zeroed cpu_set_t with one bit set, for this thread.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_SET(cpu, &mut set);
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
    }
}

/// A directory for the session's shared files: in /dev/shm when there is
/// one, else the system's temporary directory.
pub fn session_dir() -> Result<std::path::PathBuf, String> {
    let base = if Path::new("/dev/shm").is_dir() { std::path::PathBuf::from("/dev/shm") } else { std::env::temp_dir() };
    let mut seed = [0u8; 8];
    getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
    let dir = base.join(format!("terse-bus-{}-{:016x}", std::process::id(), u64::from_le_bytes(seed)));
    std::fs::create_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Packets go both ways through a bus file, sleeping and spinning.
    #[test]
    fn shared_memory_hand_over() {
        let dir = session_dir().unwrap();
        let path = dir.join("bus");
        let m = Mapping::create(&path, BUS_FILE).unwrap();
        init_bus(&m);
        let (a, b) = UnixStream::pair().unwrap();
        let mut core = ShmEnd::new(m, true, Duration::from_micros(50), a).unwrap();
        // The driver maps the file before the core's first packet, as after
        // the handshake.
        let mut d = ShmEnd::new(Mapping::open(&path, true).unwrap(), false, Duration::ZERO, b).unwrap();
        let driver = std::thread::spawn(move || {
            for _ in 0..1000 {
                let p = d.recv().unwrap().to_vec();
                let reply: Vec<u8> = p.iter().map(|x| x.wrapping_add(1)).collect();
                d.send(&reply).unwrap();
            }
        });
        for i in 0..1000u32 {
            let p = i.to_le_bytes();
            core.send(&p).unwrap();
            let r = core.recv().unwrap();
            assert_eq!(r, p.map(|x| x.wrapping_add(1)));
        }
        driver.join().unwrap();
        // The other end gone: recv says so.
        drop(core.socket.shutdown(std::net::Shutdown::Both));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
