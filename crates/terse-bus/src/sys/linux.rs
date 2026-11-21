//! Linux: Unix sockets and `mmap` ([`super::unix`]), futexes on the
//! sequence words, `sched_setaffinity`, session files in `/dev/shm`.

use std::sync::atomic::AtomicU32;
use std::time::Duration;

pub use super::unix::*;

pub fn default_session_base() -> std::path::PathBuf {
    if std::path::Path::new("/dev/shm").is_dir() { "/dev/shm".into() } else { std::env::temp_dir() }
}

/// Futexes are always there.
pub fn address_wait_available() -> bool {
    true
}

/// Sleeps while `word` holds `expected`, at most `timeout` (a futex, not
/// private: the word is shared between processes).
pub fn address_wait(word: &AtomicU32, expected: u32, timeout: Duration) {
    let ts = libc::timespec { tv_sec: timeout.as_secs() as libc::time_t, tv_nsec: timeout.subsec_nanos() as libc::c_long };
    // SAFETY: a futex on a word of a shared mapping.
    unsafe {
        libc::syscall(libc::SYS_futex, word.as_ptr(), libc::FUTEX_WAIT, expected, &ts as *const libc::timespec, 0usize, 0u32);
    }
}

/// Wakes every process sleeping on `word`.
pub fn address_wake(word: &AtomicU32) {
    // SAFETY: as above.
    unsafe {
        libc::syscall(libc::SYS_futex, word.as_ptr(), libc::FUTEX_WAKE, i32::MAX, 0usize, 0usize, 0u32);
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
