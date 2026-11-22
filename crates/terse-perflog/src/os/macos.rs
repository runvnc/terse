//! macOS: `proc_pid_rusage` (`RUSAGE_INFO_V4`, macOS 10.15 or later) for
//! another process of the same user: its CPU time (user and system), the
//! time its threads waited for a CPU and its resident size, in Mach
//! absolute-time units converted with `mach_timebase_info` (they are not
//! nanoseconds on Apple silicon). `ri_runnable_time` is the time its
//! threads were runnable, running included: the wait is that less the
//! CPU time (as XNU's own tests read it, `tests/sched/setitimer.c`). This
//! thread's CPU time from `CLOCK_THREAD_CPUTIME_ID`; for its run-queue
//! wait, its whole process's (macOS gives no thread's), which can only
//! excuse more frames as starved, never blame more. No memory-stall
//! reading. `uname`, and `sysctl` for the memory and the CPU's name.
//!
//! Run on GitHub's macos-15 runners (Apple silicon, a virtual machine).

use crate::probe::ProcSample;

pub struct Probe {
    pid: i32,
    thread: bool,
    /// Mach absolute-time units to nanoseconds: numer / denom.
    timebase: (u64, u64),
    /// The process's counters could be read when the probe was made.
    readable: bool,
}

#[repr(C)]
struct Timebase {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    // <mach/mach_time.h> (libc's binding is deprecated in favour of a
    // crate this needs nothing else of).
    fn mach_timebase_info(info: *mut Timebase) -> i32;
}

fn timebase() -> (u64, u64) {
    let mut tb = Timebase { numer: 0, denom: 0 };
    // SAFETY: fills tb.
    if unsafe { mach_timebase_info(&mut tb) } != 0 || tb.numer == 0 || tb.denom == 0 {
        return (1, 1);
    }
    (tb.numer as u64, tb.denom as u64)
}

fn rusage(pid: i32) -> Option<libc::rusage_info_v4> {
    // SAFETY: a zeroed plain struct the call fills for flavour V4.
    unsafe {
        let mut ri: libc::rusage_info_v4 = std::mem::zeroed();
        let r = libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, &mut ri as *mut libc::rusage_info_v4 as *mut libc::rusage_info_t);
        (r == 0).then_some(ri)
    }
}

impl Probe {
    pub fn process(pid: u32) -> Probe {
        let pid = pid as i32;
        Probe { pid, thread: false, timebase: timebase(), readable: rusage(pid).is_some() }
    }

    pub fn this_thread() -> Probe {
        let pid = std::process::id() as i32;
        Probe { pid, thread: true, timebase: timebase(), readable: rusage(pid).is_some() }
    }

    fn ns(&self, ticks: u64) -> u64 {
        (ticks as u128 * self.timebase.0 as u128 / self.timebase.1 as u128) as u64
    }

    /// CPU time and run-queue wait (runnable, not running), in ns.
    fn cpu_and_wait(&self, ri: &libc::rusage_info_v4) -> (u64, u64) {
        let cpu = ri.ri_user_time + ri.ri_system_time;
        (self.ns(cpu), self.ns(ri.ri_runnable_time.saturating_sub(cpu)))
    }

    /// Whether the CPU time can be read (the process's, all its threads:
    /// macOS does not give one thread of another process).
    pub fn has_cpu(&self) -> bool {
        self.readable
    }

    /// Whether the run-queue wait can be read (`ri_runnable_time` less the
    /// CPU time; for this thread, its whole process's, which is at least
    /// the thread's).
    pub fn has_wait(&self) -> bool {
        self.readable
    }

    pub fn sample(&mut self) -> Option<ProcSample> {
        if self.thread {
            let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
            // SAFETY: fills ts.
            if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) } != 0 {
                return None;
            }
            let ns = ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64;
            let wait_ns = rusage(self.pid).map_or(0, |ri| self.cpu_and_wait(&ri).1);
            return Some(ProcSample { cpu_ns: ns, run_ns: ns, wait_ns });
        }
        let ri = rusage(self.pid)?;
        let (cpu_ns, wait_ns) = self.cpu_and_wait(&ri);
        Some(ProcSample { cpu_ns, run_ns: cpu_ns, wait_ns })
    }

    pub fn rss_kib(&self) -> Option<u64> {
        rusage(self.pid).map(|ri| ri.ri_resident_size / 1024)
    }
}

/// No memory-stall reading on macOS.
pub struct Stall;

impl Stall {
    pub fn new() -> Stall {
        Stall
    }

    pub fn memory_full_ns(&self) -> Option<u64> {
        None
    }
}

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

fn sysctl(name: &str, buf: &mut [u8]) -> Option<usize> {
    let name = std::ffi::CString::new(name).ok()?;
    let mut len = buf.len();
    // SAFETY: reads at most len bytes into buf.
    let r = unsafe { libc::sysctlbyname(name.as_ptr(), buf.as_mut_ptr() as *mut libc::c_void, &mut len, std::ptr::null_mut(), 0) };
    (r == 0).then_some(len)
}

pub fn ram_kib() -> Option<u64> {
    let mut b = [0u8; 8];
    (sysctl("hw.memsize", &mut b)? == 8).then(|| u64::from_ne_bytes(b) / 1024)
}

pub fn cpu_model() -> Option<String> {
    let mut b = [0u8; 256];
    let n = sysctl("machdep.cpu.brand_string", &mut b)?;
    let s = String::from_utf8_lossy(&b[..n]);
    let s = s.trim_end_matches('\0').trim();
    (!s.is_empty()).then(|| s.to_string())
}
