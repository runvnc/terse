//! What the operating system says about a process: the CPU time it used
//! (all its threads), the time its threads were runnable but not running
//! (they waited for a CPU: the computer was busy), and its resident
//! memory; and the system's memory stall. Each system reads what it can
//! ([`crate::os`], one file per system): Linux all of it; macOS another
//! process's CPU time and run-queue wait, but neither the frame loop
//! thread's wait nor a memory stall; Windows CPU time and memory, and the
//! wait **inferred** ([`infer`]) from the CPU time and how idle the
//! system's processors were over the same interval. A reading the system
//! does not give is `None` (`has_wait` false), and a host then judges as
//! `RECORD.md` section 4 says for that case.
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
    /// Time any of its threads spent runnable, waiting for a CPU. Where
    /// the system does not say and the wait is inferred (`cpus` > 0): 0 in
    /// a reading, the inferred wait in a difference ([`Self::since`]).
    pub wait_ns: u64,
    /// Where the wait is inferred: when the reading was taken (this
    /// process's monotonic clock), and the time all the system's
    /// processors spent idle, summed. Otherwise 0.
    pub at_ns: u64,
    pub idle_ns: u64,
    /// Where the wait is inferred: the system's processors. Otherwise 0.
    pub cpus: u32,
}

impl ProcSample {
    /// What changed since `earlier` (counters only grow; a thread that
    /// ended takes its share with it, so a difference is never negative).
    /// Where the wait is inferred, the difference's `wait_ns` is
    /// [`infer`]'s over the interval between the two readings.
    pub fn since(&self, earlier: &ProcSample) -> ProcSample {
        let mut d = ProcSample {
            cpu_ns: self.cpu_ns.saturating_sub(earlier.cpu_ns),
            run_ns: self.run_ns.saturating_sub(earlier.run_ns),
            wait_ns: self.wait_ns.saturating_sub(earlier.wait_ns),
            at_ns: self.at_ns.saturating_sub(earlier.at_ns),
            idle_ns: self.idle_ns.saturating_sub(earlier.idle_ns),
            cpus: self.cpus.min(earlier.cpus),
        };
        if d.inferred() {
            d.wait_ns = infer(d.at_ns, d.cpu_ns, d.idle_ns, d.cpus).starved_ns;
        }
        d
    }

    /// Whether the wait is inferred ([`infer`]), not read.
    pub fn inferred(&self) -> bool {
        self.cpus > 0
    }

    /// Of a difference: its wait for a CPU within the `want_ns` of the
    /// interval during which it should have been running (the time the
    /// frame loop waited for a driver; the frame loop's own time, less its
    /// waits for drivers, during which it is blocked by design). A wait
    /// the system read is returned as it is (it leaves out blocked time
    /// already); an inferred one is [`infer`]'s over `want_ns`, with the
    /// system's idle time taken as spread evenly over the interval.
    pub fn wait_within(&self, want_ns: u64) -> u64 {
        if !self.inferred() || self.at_ns == 0 {
            return self.wait_ns;
        }
        let idle = (self.idle_ns as u128 * want_ns as u128 / self.at_ns as u128) as u64;
        infer(want_ns, self.cpu_ns, idle, self.cpus).starved_ns
    }
}

/// What a suspect was doing while it was waited for, as [`infer`] judges
/// it where the system does not say how long it waited for a CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Doing {
    /// On a CPU for at least half the interval: its own doing.
    Running,
    /// Little on a CPU while the system had less than one processor's
    /// worth of idle time: the computer was busy.
    Starved,
    /// Little on a CPU while the system had a processor's worth of idle
    /// time or more: blocked on something of its own (a sleep, a disk, a
    /// lock), as Linux would say.
    Blocked,
}

/// [`infer`]'s conclusion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inference {
    pub doing: Doing,
    /// The time taken as its wait for a CPU: 0 unless `Starved`.
    pub starved_ns: u64,
}

/// The rule of inference where the system gives a suspect's CPU time but
/// not its run-queue wait (Windows; `RECORD.md` 4, `PERFORMANCE.md` R14).
/// Over `interval_ns` during which the suspect (a thread, or a driver's
/// process) should have been running, it used `cpu_ns` of CPU time, and
/// the system's `cpus` processors were idle for `idle_ns` in all:
///
/// 1. `cpu_ns` at least half the interval: [`Doing::Running`]. It was
///    running, not waiting: nothing is excused.
/// 2. Else, at least one processor's worth of idle time (`idle_ns` ≥
///    `interval_ns`): [`Doing::Blocked`]. The system gives a thread that
///    is ready to run an idle processor, so the time it was not running it
///    was blocked on something of its own: nothing is excused.
/// 3. Else [`Doing::Starved`]: the computer was busy. The excuse is no
///    longer than the wait could have lasted: the interval less its time
///    on a CPU, and less the time some processor must have been idle
///    (`idle_ns / cpus`, were they all idle at once).
///
/// A pure function of its inputs, the same on every system.
pub fn infer(interval_ns: u64, cpu_ns: u64, idle_ns: u64, cpus: u32) -> Inference {
    let cpus = cpus.max(1) as u64;
    // No more idle time than every processor idle all the time.
    let idle_ns = idle_ns.min(interval_ns.saturating_mul(cpus));
    if cpu_ns.saturating_mul(2) >= interval_ns {
        Inference { doing: Doing::Running, starved_ns: 0 }
    } else if idle_ns >= interval_ns {
        Inference { doing: Doing::Blocked, starved_ns: 0 }
    } else {
        let off_cpu = interval_ns - cpu_ns;
        let none_idle = interval_ns - idle_ns / cpus;
        Inference { doing: Doing::Starved, starved_ns: off_cpu.min(none_idle) }
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
    /// macOS for another process) or inferred (Windows, [`infer`]).
    pub fn has_wait(&self) -> bool {
        self.inner.as_ref().is_some_and(os::Probe::has_wait)
    }

    /// Whether the wait is inferred ([`infer`]) rather than read.
    pub fn infers_wait(&self) -> bool {
        self.has_wait() && cfg!(windows)
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
        // wall time; without readings, 20 ms of wall time). Windows without
        // its cycle counts charges a clock tick at a time: until it has
        // been charged any.
        let enough = |tb: Option<ProcSample>| match (ta, tb) {
            (Some(ta), Some(tb)) if cfg!(windows) && !ta.inferred() => tb.since(&ta).cpu_ns > 0,
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
        if cfg!(windows) && t.has_wait() {
            // Cycle counts at their measured rate: the thread's CPU time is
            // what it worked for and, within the rate's measure (1 %), no
            // more than the time that passed; the process's at least the
            // thread's, less a clock tick (another thread's running
            // cycles may be counted at its next tick).
            let thread = tb.since(&ta).cpu_ns;
            assert!(ta.inferred() && a.inferred(), "{a:?} {ta:?}");
            assert!(thread >= 20_000_000 && thread <= wall + wall / 50, "{ta:?} {tb:?} {wall}");
            assert!(b.since(&a).cpu_ns + 16_000_000 >= thread, "{a:?} {b:?} {ta:?} {tb:?}");
            assert!(p.rss_kib().unwrap() > 100);
        } else if cfg!(windows) {
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

    const MS: u64 = 1_000_000;

    #[test]
    fn the_rule_of_inference() {
        // Running: at least half the interval on a CPU, however busy the
        // system (it was running, not waiting).
        assert_eq!(infer(40 * MS, 20 * MS, 0, 8), Inference { doing: Doing::Running, starved_ns: 0 });
        assert_eq!(infer(40 * MS, 39 * MS, 300 * MS, 8), Inference { doing: Doing::Running, starved_ns: 0 });
        // Little on a CPU, a processor's worth idle or more: blocked on
        // something of its own (a sleep), nothing excused.
        assert_eq!(infer(40 * MS, MS, 40 * MS, 8), Inference { doing: Doing::Blocked, starved_ns: 0 });
        assert_eq!(infer(40 * MS, 0, 320 * MS, 8), Inference { doing: Doing::Blocked, starved_ns: 0 });
        // Little on a CPU, less than a processor's worth idle: starved,
        // excused for no more than it was off a CPU ...
        assert_eq!(infer(40 * MS, 5 * MS, 0, 8), Inference { doing: Doing::Starved, starved_ns: 35 * MS });
        // ... nor than the time no processor need have been idle: 39 ms
        // of idle time on 8 processors was at least 4.875 ms of some idle
        // processor (all of them idle at once).
        assert_eq!(infer(40 * MS, 0, 39 * MS, 8), Inference { doing: Doing::Starved, starved_ns: 40 * MS - 39 * MS / 8 });
        // On one processor idle time is time nobody ran: 30 ms idle in 40
        // leaves at most 10 ms the suspect could have waited.
        assert_eq!(infer(40 * MS, MS, 30 * MS, 1), Inference { doing: Doing::Starved, starved_ns: 10 * MS });
        // The thresholds: exactly half on a CPU is running; exactly one
        // processor's worth idle is blocked.
        assert_eq!(infer(40 * MS, 20 * MS - 1, 0, 4).doing, Doing::Starved);
        assert_eq!(infer(40 * MS, 0, 40 * MS - 1, 4).doing, Doing::Starved);
        assert_eq!(infer(40 * MS, 0, 40 * MS, 4).doing, Doing::Blocked);
        // An idle reading above every processor idle all the time is
        // taken as that; a zero processor count as one; no interval is
        // nothing to excuse.
        assert_eq!(infer(40 * MS, 0, u64::MAX, 2).doing, Doing::Blocked);
        assert_eq!(infer(40 * MS, MS, 10 * MS, 0), infer(40 * MS, MS, 10 * MS, 1));
        assert_eq!(infer(0, 0, 0, 4), Inference { doing: Doing::Running, starved_ns: 0 });
        // Never more than the interval, whatever the inputs.
        for (i, c, idle, n) in [(17 * MS, 0, 0, 1), (17 * MS, 3, 5, 64), (1, 0, 0, 2), (u64::MAX / 2, 1, 1, 3)] {
            assert!(infer(i, c, idle, n).starved_ns <= i - c);
        }
    }

    #[test]
    fn inferred_waits_in_differences() {
        // Two readings 40 ms apart on 4 processors: the suspect ran 2 ms;
        // the processors were idle 8 ms in all (less than one's worth).
        let a = ProcSample { cpu_ns: 100 * MS, run_ns: 100 * MS, wait_ns: 0, at_ns: 1000 * MS, idle_ns: 500 * MS, cpus: 4 };
        let b = ProcSample { cpu_ns: 102 * MS, run_ns: 102 * MS, wait_ns: 0, at_ns: 1040 * MS, idle_ns: 508 * MS, cpus: 4 };
        let d = b.since(&a);
        assert!(d.inferred());
        assert_eq!((d.at_ns, d.idle_ns, d.cpu_ns), (40 * MS, 8 * MS, 2 * MS));
        // Off a CPU 38 ms; some processor idle for at least 2 ms of the 40.
        assert_eq!(d.wait_ns, 38 * MS);
        // Within the 30 ms it was waited for (its share of the idle time,
        // 6 ms, still under one processor's worth): 28 ms.
        assert_eq!(d.wait_within(30 * MS), 28 * MS);
        // Within 4 ms it was running (2 ms of 4).
        assert_eq!(d.wait_within(4 * MS), 0);
        // With 200 ms idle in the 40 ms (5 processors' worth on 8, say):
        // blocked, nothing excused.
        let b2 = ProcSample { idle_ns: 700 * MS, cpus: 8, ..b };
        let a2 = ProcSample { cpus: 8, ..a };
        assert_eq!(b2.since(&a2).wait_ns, 0);
        assert_eq!(b2.since(&a2).wait_within(30 * MS), 0);
        // A wait the system read (Linux, macOS) is kept as it is.
        let a = ProcSample { cpu_ns: 10, run_ns: 10, wait_ns: 5 * MS, ..Default::default() };
        let b = ProcSample { cpu_ns: 20, run_ns: 20, wait_ns: 9 * MS, ..Default::default() };
        let d = b.since(&a);
        assert!(!d.inferred());
        assert_eq!((d.wait_ns, d.wait_within(MS)), (4 * MS, 4 * MS));
    }

    /// Windows: a thread that spins and one that sleeps, read as the
    /// frame loop's thread and its process are. Whatever the load, the
    /// spinning thread is never taken to have been blocked, nor the
    /// sleeping one to have been running, and the readings agree with
    /// themselves (CPU time no more than the time passed, idle time no
    /// more than every processor's).
    #[cfg(windows)]
    #[test]
    fn windows_readings_classify_a_spinning_and_a_sleeping_thread() {
        let mut probe = ProcProbe::this_thread();
        if !probe.has_wait() {
            terse_testkit::skip!("cycles", "the cycle counts could not be read or their rate measured");
            return;
        }
        assert!(probe.infers_wait());
        let check = |d: ProcSample| {
            assert!(d.inferred() && d.cpus > 0, "{d:?}");
            assert!(d.cpu_ns <= d.at_ns + d.at_ns / 50 + MS, "{d:?}");
            assert!(d.idle_ns <= (d.at_ns + d.at_ns / 50 + MS) * d.cpus as u64, "{d:?}");
            infer(d.at_ns, d.cpu_ns, d.idle_ns, d.cpus)
        };
        // Spinning for 100 ms of its own CPU time (however long that takes).
        let spin = std::thread::spawn(|| {
            let mut t = ProcProbe::this_thread();
            let mut p = ProcProbe::process(std::process::id());
            let (a, pa) = (t.sample().unwrap(), p.sample().unwrap());
            let start = std::time::Instant::now();
            let mut x = 0u64;
            while t.sample().unwrap().since(&a).cpu_ns < 100 * MS && start.elapsed() < std::time::Duration::from_secs(30) {
                for _ in 0..10_000 {
                    x = x.wrapping_add(std::hint::black_box(1));
                }
            }
            assert!(x > 0);
            let (b, pb) = (t.sample().unwrap(), p.sample().unwrap());
            (b.since(&a), pb.since(&pa))
        });
        let (d, pd) = spin.join().unwrap();
        assert!(d.cpu_ns >= 100 * MS, "{d:?}");
        let i = check(d);
        assert_ne!(i.doing, Doing::Blocked, "a spinning thread: {d:?} {i:?}");
        // Its process: at least the thread's CPU time, less a clock tick.
        assert!(pd.cpu_ns + 16 * MS >= d.cpu_ns, "{pd:?} {d:?}");
        assert_ne!(infer(pd.at_ns, pd.cpu_ns, pd.idle_ns, pd.cpus).doing, Doing::Blocked, "{pd:?}");
        // Sleeping 100 ms.
        let a = probe.sample().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let d = probe.sample().unwrap().since(&a);
        assert!(d.at_ns >= 90 * MS && d.cpu_ns < 20 * MS, "{d:?}");
        let i = check(d);
        assert_ne!(i.doing, Doing::Running, "a sleeping thread: {d:?} {i:?}");
        // What the wait is taken as: all of its time off a CPU if the
        // system was busy, nothing if it had a processor to spare.
        match i.doing {
            Doing::Blocked => assert_eq!(d.wait_ns, 0),
            _ => assert!(d.wait_ns > 0 && d.wait_ns <= d.at_ns - d.cpu_ns, "{d:?}"),
        }
    }
}
