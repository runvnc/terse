//! What the operating system says about a process: the CPU time it used
//! (all its threads), the time its threads were runnable but not running
//! (they waited for a CPU: the computer was busy), and its resident
//! memory. Linux: the process's CPU-time clock (`clock_getcpuclockid`),
//! `/proc/<pid>/task/<tid>/schedstat` (time on a CPU, time waiting on a
//! run queue) and `/proc/<pid>/statm`; and the system's memory pressure
//! (`/proc/pressure/memory`, PSI). Elsewhere every reading is `None`, and
//! a host blames by its waits alone (`RECORD.md`, what a reading means).
//!
//! Each reading is a few system calls on files kept open (`pread` at
//! offset 0 regenerates a `/proc` file); `PERFORMANCE.md` gives the cost.

use std::fs::File;
use std::os::unix::fs::FileExt;

/// Cumulative counters of a process, in nanoseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcSample {
    /// CPU time of all its threads.
    pub cpu_ns: u64,
    /// Time its main thread spent on a CPU.
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

/// How often (in samples) the list of a process's threads is read again.
const TASKS_EVERY: u32 = 60;

/// Reads one process's counters.
pub struct ProcProbe {
    pid: i32,
    clock: Option<libc::clockid_t>,
    /// The main thread's schedstat (`None`: thread-self for this thread).
    main: Option<File>,
    /// Every thread's schedstat, the main thread's first.
    tasks: Vec<File>,
    samples: u32,
    /// For this thread only: its CPU-time clock instead of the process's.
    thread: bool,
}

fn schedstat(f: &File) -> Option<(u64, u64)> {
    let mut buf = [0u8; 96];
    let n = f.read_at(&mut buf, 0).ok()?;
    let text = std::str::from_utf8(&buf[..n]).ok()?;
    let mut it = text.split_whitespace();
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

impl ProcProbe {
    /// The process `pid` (all its threads).
    pub fn process(pid: u32) -> ProcProbe {
        let pid = pid as i32;
        let mut clock: libc::clockid_t = 0;
        // SAFETY: writes the clock ID for the process pid.
        let clock = (unsafe { libc::clock_getcpuclockid(pid, &mut clock) } == 0).then_some(clock);
        let main = File::open(format!("/proc/{pid}/schedstat")).ok();
        let mut p = ProcProbe { pid, clock, main, tasks: Vec::new(), samples: 0, thread: false };
        p.list_tasks();
        p
    }

    /// The calling thread only (the host's frame loop): its CPU-time clock
    /// and its own schedstat. Must be made on that thread.
    pub fn this_thread() -> ProcProbe {
        let main = File::open("/proc/thread-self/schedstat").ok();
        ProcProbe { pid: 0, clock: Some(libc::CLOCK_THREAD_CPUTIME_ID), main, tasks: Vec::new(), samples: 0, thread: true }
    }

    fn list_tasks(&mut self) {
        if self.thread {
            return;
        }
        self.tasks.clear();
        let Ok(dir) = std::fs::read_dir(format!("/proc/{}/task", self.pid)) else { return };
        for e in dir.flatten() {
            if let Ok(f) = File::open(e.path().join("schedstat")) {
                self.tasks.push(f);
            }
        }
    }

    /// Whether the run-queue wait can be read (Linux with schedstats).
    pub fn has_wait(&self) -> bool {
        self.main.as_ref().is_some_and(|f| schedstat(f).is_some())
    }

    /// The counters now; `None` once the process has gone.
    pub fn sample(&mut self) -> Option<ProcSample> {
        self.samples += 1;
        if self.samples % TASKS_EVERY == 0 {
            self.list_tasks();
        }
        let cpu_ns = match self.clock {
            Some(c) => {
                let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
                // SAFETY: fills ts for a valid clock ID.
                if unsafe { libc::clock_gettime(c, &mut ts) } != 0 {
                    return None;
                }
                ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
            }
            None => 0,
        };
        let (run_ns, main_wait) = self.main.as_ref().and_then(schedstat).unwrap_or((0, 0));
        let wait_ns = if self.thread || self.tasks.is_empty() { main_wait } else { self.tasks.iter().filter_map(schedstat).map(|(_, w)| w).sum() };
        Some(ProcSample { cpu_ns, run_ns, wait_ns })
    }

    /// Resident memory in KiB (`None`: unknown, or the process has gone).
    pub fn rss_kib(&self) -> Option<u64> {
        let path = if self.thread { "/proc/self/statm".to_string() } else { format!("/proc/{}/statm", self.pid) };
        let text = std::fs::read_to_string(path).ok()?;
        let pages: u64 = text.split_whitespace().nth(1)?.parse().ok()?;
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
        Some(pages * page / 1024)
    }
}

/// The system's memory pressure (PSI): the total time, in nanoseconds,
/// during which every non-idle task was stalled on memory (`full`).
pub struct Pressure {
    file: Option<File>,
}

impl Default for Pressure {
    fn default() -> Self {
        Pressure { file: File::open("/proc/pressure/memory").ok() }
    }
}

impl Pressure {
    pub fn memory_full_ns(&self) -> Option<u64> {
        let mut buf = [0u8; 256];
        let n = self.file.as_ref()?.read_at(&mut buf, 0).ok()?;
        let text = std::str::from_utf8(&buf[..n]).ok()?;
        let line = text.lines().find(|l| l.starts_with("full"))?;
        let total: u64 = line.split_whitespace().find_map(|f| f.strip_prefix("total="))?.parse().ok()?;
        Some(total * 1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_and_thread() {
        let mut p = ProcProbe::process(std::process::id());
        let mut t = ProcProbe::this_thread();
        let (a, ta) = (p.sample().unwrap(), t.sample().unwrap());
        let start = std::time::Instant::now();
        let mut x = 0u64;
        while start.elapsed() < std::time::Duration::from_millis(20) {
            x = x.wrapping_add(std::hint::black_box(1));
        }
        let (b, tb) = (p.sample().unwrap(), t.sample().unwrap());
        assert!(x > 0);
        if cfg!(target_os = "linux") {
            assert!(b.since(&a).cpu_ns >= 10_000_000, "{a:?} {b:?}");
            assert!(tb.since(&ta).cpu_ns >= 10_000_000 && tb.since(&ta).cpu_ns <= 40_000_000, "{ta:?} {tb:?}");
            assert!(p.rss_kib().unwrap() > 100);
        }
    }

    #[test]
    fn a_process_that_has_gone() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let mut p = ProcProbe::process(child.id());
        child.wait().unwrap();
        assert!(p.sample().is_none() || p.rss_kib().is_none());
    }
}
