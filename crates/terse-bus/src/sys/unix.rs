//! What Linux, macOS and other Unix systems share: Unix-domain sockets
//! (the standard library's), `poll`, `mmap(MAP_SHARED)` and signals.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

pub use std::os::unix::net::{UnixListener, UnixStream};

/// A connected pair (tests).
pub fn pair() -> std::io::Result<(UnixStream, UnixStream)> {
    UnixStream::pair()
}

fn poll_in(sock: &UnixStream, timeout_ms: i32) -> Option<libc::c_short> {
    let mut p = libc::pollfd { fd: sock.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    // SAFETY: one pollfd.
    let n = unsafe { libc::poll(&mut p, 1, timeout_ms) };
    (n > 0).then_some(p.revents)
}

/// Whether the peer of a socket has closed it (without reading anything).
pub fn peer_gone(sock: &UnixStream) -> bool {
    let Some(revents) = poll_in(sock, 0) else { return false };
    if revents & (libc::POLLHUP | libc::POLLERR) != 0 {
        return true;
    }
    // Readable: EOF if a peek reads nothing.
    let mut b = [0u8; 1];
    // SAFETY: peek one byte.
    let r = unsafe { libc::recv(sock.as_raw_fd(), b.as_mut_ptr() as *mut libc::c_void, 1, libc::MSG_PEEK | libc::MSG_DONTWAIT) };
    r == 0
}

/// Waits until a byte (or the end of the stream) can be read, at most
/// `timeout`. Whether one can.
pub fn wait_readable(sock: &UnixStream, timeout: Duration) -> bool {
    poll_in(sock, timeout.as_millis().clamp(1, i32::MAX as u128) as i32).is_some()
}

/// Reads and drops whatever has arrived, without waiting. `false` at the
/// end of the stream (the other end has gone) or on an error.
pub fn drain(sock: &UnixStream) -> bool {
    let mut b = [0u8; 64];
    loop {
        // SAFETY: reads at most 64 bytes into b, never waiting.
        let r = unsafe { libc::recv(sock.as_raw_fd(), b.as_mut_ptr() as *mut libc::c_void, b.len(), libc::MSG_DONTWAIT) };
        if r == 0 {
            return false;
        }
        if r < 0 {
            let e = std::io::Error::last_os_error();
            return matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted);
        }
    }
}

/// Maps `len` bytes of `file` shared with every other process that maps
/// it.
pub fn map(file: &File, len: usize, writable: bool) -> Result<*mut u8, String> {
    let prot = if writable { libc::PROT_READ | libc::PROT_WRITE } else { libc::PROT_READ };
    // SAFETY: a fresh shared mapping of a file the caller holds open.
    let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, prot, libc::MAP_SHARED, file.as_raw_fd(), 0) };
    if ptr == libc::MAP_FAILED {
        return Err(format!("mmap: {}", std::io::Error::last_os_error()));
    }
    Ok(ptr as *mut u8)
}

/// Unmaps what [`map`] mapped.
///
/// # Safety
/// `ptr` and `len` are a mapping of [`map`]'s, not used after.
pub unsafe fn unmap(ptr: *mut u8, len: usize) {
    // SAFETY: the caller's promise.
    unsafe {
        libc::munmap(ptr as *mut libc::c_void, len);
    }
}

/// Makes `path` a directory only its owner can enter (mode 0700).
pub fn make_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

extern "C" fn on_signal(_: libc::c_int) {
    crate::signals::STOP.store(true, Ordering::SeqCst);
}

/// SIGINT, SIGTERM and SIGHUP set [`crate::signals::STOP`].
pub fn install_stop_handler() {
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: a handler that only stores to an atomic.
        unsafe {
            let mut a: libc::sigaction = std::mem::zeroed();
            a.sa_sigaction = on_signal as *const () as usize;
            libc::sigemptyset(&mut a.sa_mask);
            libc::sigaction(sig, &a, std::ptr::null_mut());
        }
    }
}

/// Ends the process by SIGTERM (its default action restored first: the
/// run's handler only notes a signal), so that a parent sees the signal
/// (STATE.md 6.4).
pub fn exit_by_signal() {
    // SAFETY: resets one signal's action and sends it to this process.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_DFL);
        libc::raise(libc::SIGTERM);
    }
}
