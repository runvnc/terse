//! Windows: `GetProcessTimes` for another process's CPU time (all its
//! threads), `GetThreadTimes` for this thread's, `K32GetProcessMemoryInfo`
//! for the working set. Windows does not say how long a process's threads
//! waited for a CPU, nor how long the system stalled on memory: those
//! readings are absent (`RECORD.md` section 4). `RtlGetVersion`,
//! `GlobalMemoryStatusEx` and the registry's processor name for the
//! computer.
//!
//! Checked to compile only: not yet run on Windows.

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, GetExitCodeProcess, GetProcessTimes, GetThreadTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};

use crate::probe::ProcSample;

/// GetExitCodeProcess's code for a process still running.
const STILL_ACTIVE: u32 = 259;

pub struct Probe {
    /// The process (`None`: this thread, or it could not be opened).
    process: Option<HANDLE>,
    thread: bool,
}

// A process handle is usable from any thread.
unsafe impl Send for Probe {}

fn ft(t: &FILETIME) -> u64 {
    // 100 ns units.
    ((t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64) * 100
}

impl Probe {
    pub fn process(pid: u32) -> Probe {
        // SAFETY: opens a handle we close on drop; null on failure.
        let mut h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
        if h.is_null() {
            // SAFETY: as above, without the right to read its memory figures.
            h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        }
        Probe { process: (!h.is_null()).then_some(h), thread: false }
    }

    pub fn this_thread() -> Probe {
        Probe { process: None, thread: true }
    }

    pub fn has_cpu(&self) -> bool {
        self.thread || self.process.is_some()
    }

    /// Never: Windows gives no run-queue wait.
    pub fn has_wait(&self) -> bool {
        false
    }

    pub fn sample(&mut self) -> Option<ProcSample> {
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (zero, zero, zero, zero);
        // SAFETY: four FILETIMEs filled for a valid handle.
        let ok = unsafe {
            if self.thread {
                GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u)
            } else {
                let h = self.process?;
                let mut code = 0u32;
                if GetExitCodeProcess(h, &mut code) == 0 || code != STILL_ACTIVE {
                    return None;
                }
                GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u)
            }
        };
        if ok == 0 {
            return None;
        }
        let cpu_ns = ft(&k) + ft(&u);
        Some(ProcSample { cpu_ns, run_ns: cpu_ns, wait_ns: 0 })
    }

    pub fn rss_kib(&self) -> Option<u64> {
        // SAFETY: a zeroed struct of the size given, for a valid handle.
        unsafe {
            let h = if self.thread { GetCurrentProcess() } else { self.process? };
            if !self.thread {
                let mut code = 0u32;
                if GetExitCodeProcess(h, &mut code) == 0 || code != STILL_ACTIVE {
                    return None;
                }
            }
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
