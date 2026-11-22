//! Windows. CPU time from cycle counts: `QueryThreadCycleTime` for this
//! thread, `QueryProcessCycleTime` for another process (all its threads);
//! the system's idle time from `QueryIdleProcessorCycleTimeEx` (every
//! processor's idle thread, every processor group). Both count cycles of
//! the same constant-rate counter, exact to the cycle (where
//! `GetThreadTimes` and `GetSystemTimes` advance a clock tick, 15.6 ms, at
//! a time: as long as a frame), so the counter's rate is measured once
//! per process ([`cycles_per_ns`]) to turn them into time. Windows does
//! not say how long a thread waited for a CPU: the wait is inferred from
//! the CPU time and the idle time over the same interval
//! ([`crate::probe::infer`], `RECORD.md` 4). Where the cycle counts cannot
//! be read or their rate not measured, `GetThreadTimes`/`GetProcessTimes`
//! give the CPU time and no wait is inferred, as before. No memory-stall
//! reading. `K32GetProcessMemoryInfo` for the working set;
//! `RtlGetVersion`, `GlobalMemoryStatusEx` and the registry's processor
//! name for the computer.
//!
//! Checked to compile here; run on GitHub's windows-latest runners
//! (`.github/workflows/test.yml`), whose tests check the readings.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::Threading::{
    GetActiveProcessorCount, GetActiveProcessorGroupCount, GetCurrentProcess, GetCurrentThread, GetExitCodeProcess, GetProcessTimes, GetThreadTimes, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};
use windows_sys::Win32::System::WindowsProgramming::{QueryIdleProcessorCycleTimeEx, QueryProcessCycleTime, QueryThreadCycleTime};

use crate::probe::ProcSample;

/// GetExitCodeProcess's code for a process still running.
const STILL_ACTIVE: u32 = 259;

pub struct Probe {
    /// The process (`None`: this thread, or it could not be opened).
    process: Option<HANDLE>,
    thread: bool,
    /// The system's idle time, where the wait can be inferred.
    idle: Option<Idle>,
}

// A process handle is usable from any thread.
unsafe impl Send for Probe {}

fn ft(t: &FILETIME) -> u64 {
    // 100 ns units.
    ((t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64) * 100
}

/// When a reading was taken: nanoseconds on this process's monotonic clock.
fn now_ns() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

fn thread_cycles() -> Option<u64> {
    let mut c = 0u64;
    // SAFETY: the pseudo-handle of the calling thread; fills c.
    (unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut c) } != 0).then_some(c)
}

/// The cycle counter's rate, in cycles per nanosecond, measured once per
/// process (`None`: it could not be). This thread spins for half a
/// millisecond at a time and its cycles are set against the time that
/// passed; time it was not on a CPU only lowers the figure, so the highest
/// counts, once a second spin agrees with it within 1 %. At most 16 spins
/// (8 ms), usually 3 (1.5 ms).
fn cycles_per_ns() -> Option<f64> {
    static RATE: OnceLock<Option<f64>> = OnceLock::new();
    *RATE.get_or_init(|| {
        let mut rates: Vec<f64> = Vec::new();
        for _ in 0..16 {
            let c0 = thread_cycles()?;
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_micros(500) {
                std::hint::spin_loop();
            }
            let c1 = thread_cycles()?;
            let ns = t0.elapsed().as_nanos() as f64;
            rates.push(c1.saturating_sub(c0) as f64 / ns);
            rates.sort_by(|a, b| b.total_cmp(a));
            // A counter between 50 MHz and 20 GHz.
            if rates.len() >= 2 && rates[1] >= rates[0] * 0.99 && (0.05..20.0).contains(&rates[0]) {
                return Some(rates[0]);
            }
        }
        None
    })
}

/// The system's idle cycles: every processor's, in every group.
struct Idle {
    per_ns: f64,
    /// Each group and its processors.
    groups: Vec<(u16, u32)>,
    buf: Vec<u64>,
}

impl Idle {
    fn new() -> Option<Idle> {
        let per_ns = cycles_per_ns()?;
        // SAFETY: no arguments; a count.
        let n = unsafe { GetActiveProcessorGroupCount() };
        // SAFETY: a group number below the count.
        let groups: Vec<(u16, u32)> = (0..n).map(|g| (g, unsafe { GetActiveProcessorCount(g) })).collect();
        let most = groups.iter().map(|g| g.1).max()? as usize;
        let mut idle = Idle { per_ns, groups, buf: vec![0; most] };
        idle.read()?;
        Some(idle)
    }

    fn cpus(&self) -> u32 {
        self.groups.iter().map(|g| g.1).sum()
    }

    /// Every processor's idle time so far, summed, in ns.
    fn read(&mut self) -> Option<u64> {
        let mut cycles = 0u64;
        for &(g, n) in &self.groups {
            let mut len = n * 8;
            // SAFETY: buf holds at least n u64s; len says how many bytes.
            if unsafe { QueryIdleProcessorCycleTimeEx(g, &mut len, self.buf.as_mut_ptr()) } == 0 {
                return None;
            }
            let got = (len as usize / 8).min(self.buf.len());
            cycles = self.buf[..got].iter().fold(cycles, |a, &c| a.wrapping_add(c));
        }
        Some((cycles as f64 / self.per_ns) as u64)
    }
}

impl Probe {
    pub fn process(pid: u32) -> Probe {
        // SAFETY: opens a handle we close on drop; null on failure.
        let mut h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
        if h.is_null() {
            // SAFETY: as above, without the right to read its memory figures.
            h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        }
        let mut p = Probe { process: (!h.is_null()).then_some(h), thread: false, idle: None };
        if p.cycles().is_some() {
            p.idle = Idle::new();
        }
        p
    }

    pub fn this_thread() -> Probe {
        Probe { process: None, thread: true, idle: thread_cycles().and_then(|_| Idle::new()) }
    }

    pub fn has_cpu(&self) -> bool {
        self.thread || self.process.is_some()
    }

    /// Whether the wait can be inferred: the cycle counts can be read and
    /// their rate was measured.
    pub fn has_wait(&self) -> bool {
        self.has_cpu() && self.idle.is_some()
    }

    /// Its cycles so far (`None`: unreadable).
    fn cycles(&self) -> Option<u64> {
        if self.thread {
            return thread_cycles();
        }
        let mut c = 0u64;
        // SAFETY: our process handle (query right); fills c.
        (unsafe { QueryProcessCycleTime(self.process?, &mut c) } != 0).then_some(c)
    }

    /// Whether the process (not this thread) has ended.
    fn gone(&self) -> bool {
        let Some(h) = self.process.filter(|_| !self.thread) else { return false };
        let mut code = 0u32;
        // SAFETY: our process handle; fills code.
        unsafe { GetExitCodeProcess(h, &mut code) == 0 || code != STILL_ACTIVE }
    }

    pub fn sample(&mut self) -> Option<ProcSample> {
        if !self.thread && (self.process.is_none() || self.gone()) {
            return None;
        }
        if self.idle.is_some() {
            let cycles = self.cycles()?;
            let idle = self.idle.as_mut()?;
            let idle_ns = idle.read()?;
            let cpu_ns = (cycles as f64 / idle.per_ns) as u64;
            return Some(ProcSample { cpu_ns, run_ns: cpu_ns, wait_ns: 0, at_ns: now_ns(), idle_ns, cpus: idle.cpus() });
        }
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (zero, zero, zero, zero);
        // SAFETY: four FILETIMEs filled for a valid handle.
        let ok = unsafe {
            if self.thread {
                GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u)
            } else {
                GetProcessTimes(self.process?, &mut c, &mut e, &mut k, &mut u)
            }
        };
        if ok == 0 {
            return None;
        }
        let cpu_ns = ft(&k) + ft(&u);
        Some(ProcSample { cpu_ns, run_ns: cpu_ns, ..Default::default() })
    }

    pub fn rss_kib(&self) -> Option<u64> {
        if self.gone() {
            return None;
        }
        // SAFETY: a zeroed struct of the size given, for a valid handle.
        unsafe {
            let h = if self.thread { GetCurrentProcess() } else { self.process? };
            let mut m: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            m.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            (K32GetProcessMemoryInfo(h, &mut m, m.cb) != 0).then(|| m.WorkingSetSize as u64 / 1024)
        }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        if let Some(h) = self.process.take() {
            // SAFETY: our own handle.
            unsafe {
                CloseHandle(h);
            }
        }
    }
}

/// No memory-stall reading on Windows.
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
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
    // SAFETY: a zeroed struct with its size set, filled by the call.
    let v = unsafe {
        let mut v: OSVERSIONINFOW = std::mem::zeroed();
        v.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
        (RtlGetVersion(&mut v) == 0).then(|| format!("{}.{}.{}", v.dwMajorVersion, v.dwMinorVersion, v.dwBuildNumber))
    };
    ("Windows".to_string(), v.unwrap_or_default())
}

pub fn ram_kib() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: a zeroed struct with its length set, filled by the call.
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        (GlobalMemoryStatusEx(&mut m) != 0).then(|| m.ullTotalPhys / 1024)
    }
}

pub fn cpu_model() -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (key, value) = (wide(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"), wide("ProcessorNameString"));
    let mut buf = [0u16; 256];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: reads at most len bytes into buf.
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut core::ffi::c_void, &mut len) };
    if r != 0 {
        return None;
    }
    let n = (len as usize / 2).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..n]);
    let s = s.trim_end_matches('\0').trim();
    (!s.is_empty()).then(|| s.to_string())
}
