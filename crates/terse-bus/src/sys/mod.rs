//! What the bus needs of the operating system (DEVICES.md 4.3), one file
//! per system with the same interface:
//!
//! - **The socket** (DEVICES.md 2, 3): [`UnixListener`] and [`UnixStream`],
//!   a stream socket at a path. On Linux and macOS the standard library's;
//!   on Windows an `AF_UNIX` socket through Winsock (Windows 10 1803 or
//!   later), which the standard library does not expose. With it:
//!   [`peer_gone`] (the other end closed it, without reading anything),
//!   [`wait_readable`] (a byte or the end arrived, or a time passed) and
//!   [`drain`] (reads what has arrived without waiting).
//! - **Shared memory** (DEVICES.md 4.1, 4.2): [`map`] and [`unmap`] a whole
//!   file, shared between processes; [`session_base`], where a run's
//!   files go; [`make_private_dir`].
//! - **Sleep and wake** on a 32-bit word of a shared mapping, between
//!   processes ([`address_wait`], [`address_wake`]): Linux futexes; on
//!   macOS `os_sync_wait_on_address` (14.4 or later) or `__ulock_wait`
//!   (10.12 or later), shared flavours, looked up when first needed; none on
//!   Windows ([`address_wait_available`] false: an end sleeps on the
//!   socket instead, DEVICES.md 4.2).
//! - [`pin_to`] a CPU (Linux, Windows; nothing on macOS), the signals that
//!   end a run cleanly ([`install_stop_handler`]) and [`exit_by_signal`].

#[cfg(unix)]
mod unix;

#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(target_os = "macos", path = "macos.rs")]
#[cfg_attr(windows, path = "windows.rs")]
#[cfg_attr(all(unix, not(any(target_os = "linux", target_os = "macos"))), path = "other_unix.rs")]
mod imp;

pub use imp::*;

/// Where a core puts its session directory unless `TERSE_BUS_DIR` names
/// another: `/dev/shm` on Linux (a tmpfs: the shared files never touch a
/// disk), else a short temporary directory (a socket's path must fit in
/// 104 bytes on macOS).
pub fn session_base() -> std::path::PathBuf {
    match std::env::var_os("TERSE_BUS_DIR") {
        Some(d) if !d.is_empty() => d.into(),
        _ => imp::default_session_base(),
    }
}
