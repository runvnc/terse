//! SIGINT, SIGTERM and SIGHUP end a run cleanly (DEVICES.md 10): the
//! handler only notes the signal; the frame loop sees it at the next frame
//! and shuts the drivers down, so that none is left behind.

use std::sync::atomic::{AtomicBool, Ordering};

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// Installs the handler.
pub fn install() {
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

/// Whether a signal asked the run to stop.
pub fn stop_requested() -> bool {
    STOP.load(Ordering::SeqCst)
}
