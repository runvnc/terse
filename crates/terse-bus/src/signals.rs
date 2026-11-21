//! SIGINT, SIGTERM and SIGHUP (Windows: Ctrl+C, Ctrl+Break, closing the
//! console) end a run cleanly (DEVICES.md 10): the handler only notes the
//! request; the frame loop sees it at the next frame and shuts the drivers
//! down, so that none is left behind.

use std::sync::atomic::{AtomicBool, Ordering};

/// Set by the handler ([`crate::sys::install_stop_handler`]).
pub(crate) static STOP: AtomicBool = AtomicBool::new(false);

/// Installs the handler.
pub fn install() {
    crate::sys::install_stop_handler();
}

/// Whether a signal asked the run to stop.
pub fn stop_requested() -> bool {
    STOP.load(Ordering::SeqCst)
}
