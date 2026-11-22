//! What the operating system says about a process: the CPU time it used
//! (all its threads), the time its threads were runnable but not running
//! (they waited for a CPU: the computer was busy), and its resident
//! memory; and the system's memory stall. Each system reads what it can
//! ([`crate::os`], one file per system): Linux all of it; macOS another
//! process's CPU time and run-queue wait, but neither the frame loop
//! thread's wait nor a memory stall; Windows CPU time and memory only. A
//! reading the system does not give is `None` (`has_wait` false), and a
//! host then judges as `RECORD.md` section 4 says for that case.
//!
//! `TERSE_PERF_PROBE=none` turns every reading off, as on a system that
//! gives none: for testing that case anywhere.

use crate::os;

/// Cumulative counters of a process, in nanoseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcSample {
    /// CPU time of all its threads.
    pub cpu_ns: u64,
    /// Time its main thread spent on a CPU (where the system gives only
    /// the process's CPU time: that).
    pub run_ns: u64,
    /// Time any of its threads spent runnable, waiting for a CPU.
    pub wait_ns: u64,
}

impl ProcSample {
    /// What changed since `earlier` (counters only grow; a thread that
    /// ended takes its share with it, so a difference is never negative).
    pub fn since(&self, earlier: &ProcSample) -> ProcSample {
        ProcSample {
            cpu_ns: self.cpu_ns.saturating_sub(earlier.cpu_ns),
            run_ns: self.run_ns.saturating_sub(earlier.run_ns),
            wait_ns: self.wait_ns.saturating_sub(earlier.wait_ns),
        }
    }
}

/// Whether readings are turned off (`TERSE_PERF_PROBE=none`).
pub fn disabled() -> bool {
    std::env::var("TERSE_PERF_PROBE").is_ok_and(|v| v == "none")
}

/// Reads one process's counters.
pub struct ProcProbe {
    inner: Option<os::Probe>,
}

impl ProcProbe {
    /// The process `pid` (all its threads).
    pub fn process(pid: u32) -> ProcProbe {
        ProcProbe { inner: (!disabled()).then(|| os::Probe::process(pid)) }
    }

    /// The calling thread only (the host's frame loop): its CPU time and,
    /// where the system says, its run-queue wait. Must be made on that
    /// thread.
    pub fn this_thread() -> ProcProbe {
        ProcProbe { inner: (!disabled()).then(os::Probe::this_thread) }
    }

    /// Whether its time on a CPU can be read.
    pub fn has_cpu(&self) -> bool {
        self.inner.as_ref().is_some_and(os::Probe::has_cpu)
    }

    /// Whether the run-queue wait can be read (Linux with schedstats;
    /// macOS for another process).
    pub fn has_wait(&self) -> bool {
        self.inner.as_ref().is_some_and(os::Probe::has_wait)
    }

    /// The counters now; `None` once the process has gone, or if nothing
    /// can be read.
    pub fn sample(&mut self) -> Option<ProcSample> {
        self.inner.as_mut()?.sample()
    }

    /// Resident memory in KiB (`None`: unknown, or the process has gone).
    pub fn rss_kib(&self) -> Option<u64> {
        self.inner.as_ref()?.rss_kib()
    }
}

/// The system's memory pressure: the total time, in nanoseconds, during
/// which every non-idle task was stalled on memory (Linux PSI `full`).
pub struct Pressure {
    inner: Option<os::Stall>,
}

impl Default for Pressure {
    fn default() -> Self {
        Pressure { inner: (!disabled()).then(os::Stall::new) }
    }
}

impl Pressure {
    pub fn memory_full_ns(&self) -> Option<u64> {
        self.inner.as_ref()?.memory_full_ns()
    }

    /// Whether the system says (Linux with PSI).
    pub fn available(&self) -> bool {
        self.memory_full_ns().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_and_thread() {
        let mut p = ProcProbe::process(std::process::id());
        let mut t = ProcProbe::this_thread();
        let start = std::time::Instant::now();
        let (a, ta) = (p.sample(), t.sample());
        let mut x = 0u64;
        // Work until this thread has had 20 ms of a CPU by its own clock
        // (on a busy computer that may take much longer than 20 ms of
        // wall time; without readings, 20 ms of wall time). Windows charges
        // a clock tick at a time: until it has been charged any.
        let enough = |tb: Option<ProcSample>| match (ta, tb) {
            (Some(ta), Some(tb)) if cfg!(windows) => tb.since(&ta).cpu_ns > 0,
            (Some(ta), Some(tb)) => tb.since(&ta).cpu_ns >= 20_000_000,
            _ => start.elapsed() >= std::time::Duration::from_millis(20),
        };
        while !enough(t.sample()) && start.elapsed() < std::time::Duration::from_secs(20) {
            for _ in 0..10_000 {
                x = x.wrapping_add(std::hint::black_box(1));
            }
        }
        let tb = t.sample();
        let wall = start.elapsed().as_nanos() as u64;
        let b = p.sample();
        assert!(x > 0);
        if disabled() {
            assert!(a.is_none() && b.is_none() && ta.is_none() && tb.is_none() && !p.has_wait() && !p.has_cpu());
            return;
        }
        let (a, b, ta, tb) = (a.unwrap(), b.unwrap(), ta.unwrap(), tb.unwrap());
        if cfg!(windows) {
            // Charged a clock tick (15.6 ms) at a time.
            assert!(b.since(&a).cpu_ns > 0 && tb.since(&ta).cpu_ns > 0, "{a:?} {b:?} {ta:?} {tb:?}");
            assert!(p.rss_kib().unwrap() > 100);
        } else if cfg!(any(target_os = "linux", target_os = "macos")) {
            // The thread's CPU time: what it worked for, never more than
            // the time that passed; the process's: at least the thread's.
            let thread = tb.since(&ta).cpu_ns;
            assert!(thread >= 20_000_000 && thread <= wall, "{ta:?} {tb:?} {wall}");
            assert!(b.since(&a).cpu_ns >= thread, "{a:?} {b:?} {ta:?} {tb:?}");
            assert!(p.rss_kib().unwrap() > 100);
        }
    }

    #[test]
    fn a_process_that_has_gone() {
        // This test binary itself, listing its tests: a process that ends
        // at once, on any system.
        let exe = std::env::current_exe().unwrap();
        let mut child = std::process::Command::new(exe).arg("--list").stdout(std::process::Stdio::null()).spawn().unwrap();
        let mut p = ProcProbe::process(child.id());
        child.wait().unwrap();
        assert!(p.sample().is_none() || p.rss_kib().is_none());
    }
}
