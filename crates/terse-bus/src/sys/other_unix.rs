//! Another Unix system: Unix sockets and `mmap` ([`super::unix`]); no
//! cross-process sleep on an address is used, so an end sleeps on the
//! socket (DEVICES.md 4.2); no CPU affinity.

use std::sync::atomic::AtomicU32;
use std::time::Duration;

pub use super::unix::*;

pub fn default_session_base() -> std::path::PathBuf {
    "/tmp".into()
}

pub fn address_wait_available() -> bool {
    false
}

pub fn address_wait(_word: &AtomicU32, _expected: u32, timeout: Duration) {
    std::thread::sleep(timeout.min(Duration::from_millis(1)));
}

pub fn address_wake(_word: &AtomicU32) {}

pub fn pin_to(_cpu: usize) {}
