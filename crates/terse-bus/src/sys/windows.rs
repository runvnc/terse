//! Windows: `AF_UNIX` stream sockets through Winsock (Windows 10 1803 or
//! later; the standard library has no Unix sockets on Windows), files
//! mapped with `CreateFileMappingW` and `MapViewOfFile` (views of one file
//! are coherent between processes), session files in `%TEMP%`. There is no
//! sleep on an address across processes (`WaitOnAddress` is only within a
//! process), so an end sleeps on the socket and is woken by a byte
//! (DEVICES.md 4.2). Console control events (Ctrl+C, Ctrl+Break, closing
//! the console) end a run cleanly, as the signals do elsewhere.
//!
//! Checked to compile only: not yet run on Windows.

use std::fs::File;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Once};
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation};
use windows_sys::Win32::Networking::WinSock::{
    AF_UNIX, FIONBIO, INVALID_SOCKET, MSG_PEEK, POLLERR, POLLHUP, POLLNVAL, POLLRDNORM, SD_BOTH, SD_RECEIVE, SD_SEND, SO_RCVTIMEO, SOCK_STREAM, SOCKADDR, SOCKADDR_UN, SOCKET,
    SOCKET_ERROR, SOL_SOCKET, WSA_FLAG_NO_HANDLE_INHERIT, WSADATA, WSAGetLastError, WSAPOLLFD, WSAPoll, WSASocketW, WSAStartup, accept, bind, closesocket, connect, ioctlsocket, listen,
    recv, send, setsockopt, shutdown,
};
use windows_sys::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READONLY, PAGE_READWRITE, UnmapViewOfFile};

pub fn default_session_base() -> std::path::PathBuf {
    std::env::temp_dir()
}

fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // SAFETY: Winsock 2.2, once; never cleaned up (the process ends).
        unsafe {
            let mut d: WSADATA = std::mem::zeroed();
            WSAStartup(0x202, &mut d);
        }
    });
}

fn last_error() -> io::Error {
    // SAFETY: no preconditions.
    io::Error::from_raw_os_error(unsafe { WSAGetLastError() })
}

/// An owned socket, closed when dropped.
struct Sock(SOCKET);

impl Drop for Sock {
    fn drop(&mut self) {
        // SAFETY: our own socket.
        unsafe {
            closesocket(self.0);
        }
    }
}

fn new_socket() -> io::Result<Sock> {
    init();
    // SAFETY: a new socket, not inherited by the drivers the core starts
    // (a driver holding another's connection would hide its exit).
    let s = unsafe { WSASocketW(AF_UNIX as i32, SOCK_STREAM, 0, std::ptr::null(), 0, WSA_FLAG_NO_HANDLE_INHERIT) };
    if s == INVALID_SOCKET { Err(last_error()) } else { Ok(Sock(s)) }
}

/// The address of `path`: its UTF-8 bytes, NUL-terminated, in 108 bytes.
fn address(path: &Path) -> io::Result<(SOCKADDR_UN, i32)> {
    let bytes = path.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a socket path must be UTF-8"))?.as_bytes();
    // SAFETY: a plain struct.
    let mut a: SOCKADDR_UN = unsafe { std::mem::zeroed() };
    if bytes.len() >= a.sun_path.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{}: longer than a socket's path may be", path.display())));
    }
    a.sun_family = AF_UNIX;
    for (d, s) in a.sun_path.iter_mut().zip(bytes) {
        *d = *s as i8;
    }
    Ok((a, (std::mem::size_of::<u16>() + bytes.len() + 1) as i32))
}

fn set_nonblocking(s: SOCKET, on: bool) -> io::Result<()> {
    let mut v = on as u32;
    // SAFETY: FIONBIO with a u32.
    if unsafe { ioctlsocket(s, FIONBIO, &mut v) } == SOCKET_ERROR { Err(last_error()) } else { Ok(()) }
}

/// A listening `AF_UNIX` socket.
pub struct UnixListener(Sock);

impl UnixListener {
    pub fn bind(path: impl AsRef<Path>) -> io::Result<UnixListener> {
        let s = new_socket()?;
        let (a, len) = address(path.as_ref())?;
        // SAFETY: a valid address of the length given.
        unsafe {
            if bind(s.0, &a as *const SOCKADDR_UN as *const SOCKADDR, len) == SOCKET_ERROR || listen(s.0, 8) == SOCKET_ERROR {
                return Err(last_error());
            }
        }
        Ok(UnixListener(s))
    }

    pub fn set_nonblocking(&self, on: bool) -> io::Result<()> {
        set_nonblocking(self.0.0, on)
    }

    pub fn accept(&self) -> io::Result<(UnixStream, ())> {
        // SAFETY: no address wanted.
        let s = unsafe { accept(self.0.0, std::ptr::null_mut(), std::ptr::null_mut()) };
        if s == INVALID_SOCKET {
            return Err(last_error());
        }
        // SAFETY: a socket is a handle; never inherited by a child.
        unsafe {
            SetHandleInformation(s as HANDLE, HANDLE_FLAG_INHERIT, 0);
        }
        Ok((UnixStream(Arc::new(Sock(s))), ()))
    }
}

/// A connected `AF_UNIX` stream. Clones share the one socket (as a
/// duplicated descriptor would), which closes with the last of them.
pub struct UnixStream(Arc<Sock>);

impl UnixStream {
    pub fn connect(path: impl AsRef<Path>) -> io::Result<UnixStream> {
        let s = new_socket()?;
        let (a, len) = address(path.as_ref())?;
        // SAFETY: a valid address of the length given.
        if unsafe { connect(s.0, &a as *const SOCKADDR_UN as *const SOCKADDR, len) } == SOCKET_ERROR {
            return Err(last_error());
        }
        Ok(UnixStream(Arc::new(s)))
    }

    pub fn try_clone(&self) -> io::Result<UnixStream> {
        Ok(UnixStream(self.0.clone()))
    }

    fn raw(&self) -> SOCKET {
        self.0.0
    }

    pub fn set_nonblocking(&self, on: bool) -> io::Result<()> {
        set_nonblocking(self.raw(), on)
    }

    /// `None`: wait for ever.
    pub fn set_read_timeout(&self, d: Option<Duration>) -> io::Result<()> {
        let ms: u32 = d.map_or(0, |d| d.as_millis().clamp(1, u32::MAX as u128) as u32);
        // SAFETY: SO_RCVTIMEO takes a DWORD of milliseconds.
        if unsafe { setsockopt(self.raw(), SOL_SOCKET, SO_RCVTIMEO, &ms as *const u32 as *const u8, 4) } == SOCKET_ERROR { Err(last_error()) } else { Ok(()) }
    }

    pub fn shutdown(&self, how: std::net::Shutdown) -> io::Result<()> {
        let how = match how {
            std::net::Shutdown::Read => SD_RECEIVE,
            std::net::Shutdown::Write => SD_SEND,
            std::net::Shutdown::Both => SD_BOTH,
        };
        // SAFETY: our socket.
        if unsafe { shutdown(self.raw(), how) } == SOCKET_ERROR { Err(last_error()) } else { Ok(()) }
    }
}

impl io::Read for UnixStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(i32::MAX as usize) as i32;
        // SAFETY: at most n bytes into buf.
        let r = unsafe { recv(self.raw(), buf.as_mut_ptr(), n, 0) };
        if r == SOCKET_ERROR { Err(last_error()) } else { Ok(r as usize) }
    }
}

impl io::Write for UnixStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = buf.len().min(i32::MAX as usize) as i32;
        // SAFETY: at most n bytes from buf.
        let r = unsafe { send(self.raw(), buf.as_ptr(), n, 0) };
        if r == SOCKET_ERROR { Err(last_error()) } else { Ok(r as usize) }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A connected pair (tests): a listener in a fresh temporary directory.
pub fn pair() -> io::Result<(UnixStream, UnixStream)> {
    let mut seed = [0u8; 8];
    getrandom::fill(&mut seed).map_err(|e| io::Error::other(e.to_string()))?;
    let dir = std::env::temp_dir().join(format!("terse-pair-{:016x}", u64::from_le_bytes(seed)));
    std::fs::create_dir(&dir)?;
    let path = dir.join("s");
    let l = UnixListener::bind(&path)?;
    let a = UnixStream::connect(&path)?;
    let (b, ()) = l.accept()?;
    drop(l);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
    Ok((a, b))
}

fn poll_in(s: SOCKET, timeout_ms: i32) -> Option<i16> {
    let mut p = WSAPOLLFD { fd: s, events: POLLRDNORM, revents: 0 };
    // SAFETY: one WSAPOLLFD.
    let n = unsafe { WSAPoll(&mut p, 1, timeout_ms) };
    (n > 0).then_some(p.revents)
}

/// Whether the peer of a socket has closed it (without reading anything).
pub fn peer_gone(sock: &UnixStream) -> bool {
    let Some(revents) = poll_in(sock.raw(), 0) else { return false };
    if revents & (POLLHUP | POLLERR | POLLNVAL) != 0 {
        return true;
    }
    let mut b = [0u8; 1];
    // SAFETY: peek one byte; readable, so it does not wait.
    let r = unsafe { recv(sock.raw(), b.as_mut_ptr(), 1, MSG_PEEK) };
    r == 0 || r == SOCKET_ERROR
}

/// Waits until a byte (or the end of the stream) can be read, at most
/// `timeout`. Whether one can.
pub fn wait_readable(sock: &UnixStream, timeout: Duration) -> bool {
    poll_in(sock.raw(), timeout.as_millis().clamp(1, i32::MAX as u128) as i32).is_some()
}

/// Reads and drops whatever has arrived, without waiting. `false` at the
/// end of the stream or on an error.
pub fn drain(sock: &UnixStream) -> bool {
    let mut b = [0u8; 64];
    while let Some(revents) = poll_in(sock.raw(), 0) {
        if revents & (POLLHUP | POLLERR | POLLNVAL) != 0 && revents & POLLRDNORM == 0 {
            return false;
        }
        // SAFETY: readable, so it does not wait; at most 64 bytes.
        let r = unsafe { recv(sock.raw(), b.as_mut_ptr(), b.len() as i32, 0) };
        if r <= 0 {
            return false;
        }
    }
    true
}

/// Maps `len` bytes of `file` shared with every other process that maps
/// it.
pub fn map(file: &File, len: usize, writable: bool) -> Result<*mut u8, String> {
    let protect = if writable { PAGE_READWRITE } else { PAGE_READONLY };
    // SAFETY: a mapping of a file we hold open; the view keeps the mapping
    // object alive after its handle is closed.
    unsafe {
        let m = CreateFileMappingW(file.as_raw_handle() as HANDLE, std::ptr::null(), protect, 0, 0, std::ptr::null());
        if m.is_null() {
            return Err(format!("CreateFileMappingW: {}", io::Error::last_os_error()));
        }
        let v = MapViewOfFile(m, if writable { FILE_MAP_READ | FILE_MAP_WRITE } else { FILE_MAP_READ }, 0, 0, len);
        let e = io::Error::last_os_error();
        CloseHandle(m);
        if v.Value.is_null() {
            return Err(format!("MapViewOfFile: {e}"));
        }
        Ok(v.Value as *mut u8)
    }
}

/// Unmaps what [`map`] mapped.
///
/// # Safety
/// `ptr` is a view of [`map`]'s, not used after.
pub unsafe fn unmap(ptr: *mut u8, _len: usize) {
    // SAFETY: the caller's promise.
    unsafe {
        UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: ptr as *mut core::ffi::c_void });
    }
}

/// Creates `path`. Its access is that of the user's temporary directory
/// (private to the user), which it inherits.
pub fn make_private_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

/// No sleep on an address across processes on Windows.
pub fn address_wait_available() -> bool {
    false
}

pub fn address_wait(_word: &AtomicU32, _expected: u32, timeout: Duration) {
    std::thread::sleep(timeout.min(Duration::from_millis(1)));
}

pub fn address_wake(_word: &AtomicU32) {}

/// Pins the calling thread to `cpu` (the first 64 only).
pub fn pin_to(cpu: usize) {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadAffinityMask};
    if cpu < 64 {
        // SAFETY: this thread's affinity.
        unsafe {
            SetThreadAffinityMask(GetCurrentThread(), 1usize << cpu);
        }
    }
}

unsafe extern "system" fn on_control(kind: u32) -> windows_sys::core::BOOL {
    use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT};
    crate::signals::STOP.store(true, Ordering::SeqCst);
    if kind != CTRL_C_EVENT && kind != CTRL_BREAK_EVENT {
        // The console is closing (or the user logging off): the process
        // ends when this returns, so give the frame loop time to shut the
        // drivers down (Windows allows about 5 s).
        std::thread::sleep(Duration::from_secs(3));
    }
    1
}

/// Ctrl+C, Ctrl+Break and closing the console set
/// [`crate::signals::STOP`].
pub fn install_stop_handler() {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    // SAFETY: a handler that stores to an atomic (and may sleep).
    unsafe {
        SetConsoleCtrlHandler(Some(on_control), 1);
    }
}

/// No signals on Windows: the caller's exit status (255) says it.
pub fn exit_by_signal() {}
