//! macOS: Unix sockets and `mmap` ([`super::unix`]); session files in
//! `/tmp` (`$TMPDIR` is too long for a socket's path, which must fit in 104
//! bytes); no CPU affinity. Sleep and wake across processes on a word of a
//! shared mapping: `os_sync_wait_on_address_with_timeout` and
//! `os_sync_wake_by_address_all` with their `SHARED` flags (macOS 14.4 or
//! later), else `__ulock_wait` and `__ulock_wake` with
//! `UL_COMPARE_AND_WAIT_SHARED` (10.12 or later; what the first is built
//! on). Both are looked up when first needed, so that the program still
//! starts on a system without them; with neither, an end sleeps on the
//! socket (DEVICES.md 4.2).
//!
//! Checked to compile only: not yet run on a Mac.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

pub use super::unix::*;

pub fn default_session_base() -> std::path::PathBuf {
    "/tmp".into()
}

type OsWait = unsafe extern "C" fn(addr: *mut c_void, value: u64, size: usize, flags: u32, clock: u32, timeout_ns: u64) -> i32;
type OsWake = unsafe extern "C" fn(addr: *mut c_void, size: usize, flags: u32) -> i32;
type UlockWait = unsafe extern "C" fn(op: u32, addr: *mut c_void, value: u64, timeout_us: u32) -> i32;
type UlockWake = unsafe extern "C" fn(op: u32, addr: *mut c_void, wake_value: u64) -> i32;

/// `<os/os_sync_wait_on_address.h>`.
const OS_SYNC_WAIT_ON_ADDRESS_SHARED: u32 = 1;
const OS_SYNC_WAKE_BY_ADDRESS_SHARED: u32 = 1;
const OS_CLOCK_MACH_ABSOLUTE_TIME: u32 = 32;
/// xnu's `sys/ulock.h`.
const UL_COMPARE_AND_WAIT_SHARED: u32 = 3;
const ULF_WAKE_ALL: u32 = 0x100;
const ULF_NO_ERRNO: u32 = 0x0100_0000;

#[derive(Clone, Copy)]
enum Prims {
    OsSync(OsWait, OsWake),
    Ulock(UlockWait, UlockWake),
}

fn lookup(name: &[u8]) -> *mut c_void {
    // SAFETY: a NUL-terminated name, the default search.
    unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr() as *const libc::c_char) }
}

fn prims() -> Option<Prims> {
    static P: OnceLock<Option<Prims>> = OnceLock::new();
    *P.get_or_init(|| {
        let (w, k) = (lookup(b"os_sync_wait_on_address_with_timeout\0"), lookup(b"os_sync_wake_by_address_all\0"));
        if !w.is_null() && !k.is_null() {
            // SAFETY: the system's functions of these signatures.
            return Some(unsafe { Prims::OsSync(std::mem::transmute::<*mut c_void, OsWait>(w), std::mem::transmute::<*mut c_void, OsWake>(k)) });
        }
        let (w, k) = (lookup(b"__ulock_wait\0"), lookup(b"__ulock_wake\0"));
        if !w.is_null() && !k.is_null() {
            // SAFETY: as above.
            return Some(unsafe { Prims::Ulock(std::mem::transmute::<*mut c_void, UlockWait>(w), std::mem::transmute::<*mut c_void, UlockWake>(k)) });
        }
        None
    })
}

pub fn address_wait_available() -> bool {
    prims().is_some()
}

/// Sleeps while `word` holds `expected`, at most `timeout`.
pub fn address_wait(word: &AtomicU32, expected: u32, timeout: Duration) {
    let addr = word.as_ptr() as *mut c_void;
    // SAFETY: a wait on a word of a shared mapping, with its own value.
    unsafe {
        match prims() {
            Some(Prims::OsSync(wait, _)) => {
                wait(addr, expected as u64, 4, OS_SYNC_WAIT_ON_ADDRESS_SHARED, OS_CLOCK_MACH_ABSOLUTE_TIME, timeout.as_nanos().min(u64::MAX as u128) as u64);
            }
            Some(Prims::Ulock(wait, _)) => {
                wait(UL_COMPARE_AND_WAIT_SHARED | ULF_NO_ERRNO, addr, expected as u64, timeout.as_micros().clamp(1, u32::MAX as u128) as u32);
            }
            None => std::thread::sleep(timeout.min(Duration::from_millis(1))),
        }
    }
}

/// Wakes every process sleeping on `word`.
pub fn address_wake(word: &AtomicU32) {
    let addr = word.as_ptr() as *mut c_void;
    // SAFETY: as above.
    unsafe {
        match prims() {
            Some(Prims::OsSync(_, wake)) => {
                wake(addr, 4, OS_SYNC_WAKE_BY_ADDRESS_SHARED);
            }
            Some(Prims::Ulock(_, wake)) => {
                wake(UL_COMPARE_AND_WAIT_SHARED | ULF_WAKE_ALL | ULF_NO_ERRNO, addr, 0);
            }
            None => {}
        }
    }
}

/// macOS has no way to pin a thread: nothing.
pub fn pin_to(_cpu: usize) {}
