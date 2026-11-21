//! Linux: a process's CPU-time clock (`clock_getcpuclockid`), each thread's
//! `/proc/<pid>/task/<tid>/schedstat` (time on a CPU, time waiting on a run
//! queue), `/proc/<pid>/statm`, the system's memory pressure
//! (`/proc/pressure/memory`, PSI), `uname`, `/proc/meminfo` and
//! `/proc/cpuinfo`.
//!
//! Each reading is a few system calls on files kept open (`pread` at
//! offset 0 regenerates a `/proc` file); `PERFORMANCE.md` gives the cost.

use std::fs::File;
use std::os::unix::fs::FileExt;

use crate::probe::ProcSample;

/// How often (in samples) the list of a process's threads is read again.
const TASKS_EVERY: u32 = 60;

pub struct Probe {
    pid: i32,
    clock: Option<libc::clockid_t>,
    /// The main thread's schedstat (or this thread's).
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

impl Probe {
    pub fn process(pid: u32) -> Probe {
        let pid = pid as i32;
        let mut clock: libc::clockid_t = 0;
        // SAFETY: writes the clock ID for the process pid.
        let clock = (unsafe { libc::clock_getcpuclockid(pid, &mut clock) } == 0).then_some(clock);
        let main = File::open(format!("/proc/{pid}/schedstat")).ok();
        let mut p = Probe { pid, clock, main, tasks: Vec::new(), samples: 0, thread: false };
        p.list_tasks();
        p
    }

    pub fn this_thread() -> Probe {
        let main = File::open("/proc/thread-self/schedstat").ok();
        Probe { pid: 0, clock: Some(libc::CLOCK_THREAD_CPUTIME_ID), main, tasks: Vec::new(), samples: 0, thread: true }
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

    /// Whether the time on a CPU can be read: the waited-on thread's
    /// (schedstat), which is what [`ProcSample::run_ns`] is.
    pub fn has_cpu(&self) -> bool {
        self.has_wait()
    }

    /// Whether the run-queue wait can be read (schedstats in the kernel).
    pub fn has_wait(&self) -> bool {
        self.main.as_ref().is_some_and(|f| schedstat(f).is_some())
    }

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

    pub fn rss_kib(&self) -> Option<u64> {
        let path = if self.thread { "/proc/self/statm".to_string() } else { format!("/proc/{}/statm", self.pid) };
        let text = std::fs::read_to_string(path).ok()?;
        let pages: u64 = text.split_whitespace().nth(1)?.parse().ok()?;
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
        Some(pages * page / 1024)
    }
}

/// PSI: the total time every non-idle task was stalled on memory (`full`).
pub struct Stall {
    file: Option<File>,
}

impl Stall {
    pub fn new() -> Stall {
        Stall { file: File::open("/proc/pressure/memory").ok() }
    }

    pub fn memory_full_ns(&self) -> Option<u64> {
        let mut buf = [0u8; 256];
        let n = self.file.as_ref()?.read_at(&mut buf, 0).ok()?;
        let text = std::str::from_utf8(&buf[..n]).ok()?;
        let line = text.lines().find(|l| l.starts_with("full"))?;
        let total: u64 = line.split_whitespace().find_map(|f| f.strip_prefix("total="))?.parse().ok()?;
        Some(total * 1000)
    }
}

/// The system's name and release (the kernel's).
pub fn os_name() -> (String, String) {
    // SAFETY: uname fills a struct of fixed-size arrays it NUL-terminates.
    unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut u) != 0 {
            return (std::env::consts::OS.to_string(), String::new());
        }
        let s = |a: &[libc::c_char]| std::ffi::CStr::from_ptr(a.as_ptr()).to_string_lossy().into_owned();
        (s(&u.sysname), s(&u.release))
    }
}

/// MemTotal in KiB.
pub fn ram_kib() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    text.lines().find_map(|l| l.strip_prefix("MemTotal:"))?.split_whitespace().next()?.parse().ok()
}

/// The first CPU's model name (`model name`, or `Hardware` / `Processor`
/// on some ARM systems).
pub fn cpu_model() -> Option<String> {
    let text = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    for key in ["model name", "Hardware", "Processor", "cpu model"] {
        if let Some(v) = text.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == key).then(|| v.trim().to_string())
        }) {
            return Some(v);
        }
    }
    None
}
