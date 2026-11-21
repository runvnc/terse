//! A coarse description of the computer a record was made on (`RECORD.md`
//! `env`): enough to compare like with like, nothing that names the
//! machine or its user. No host name, user name, serial number, network
//! address or path; the RAM is rounded to whole GiB.

/// The computer, as the record's `env` line gives it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Environment {
    /// The operating system's name (`Linux`) and release (the kernel's).
    pub os: String,
    pub os_version: String,
    /// The processor architecture (`x86_64`, `aarch64`).
    pub arch: String,
    /// Logical CPUs available to the process.
    pub cores: u32,
    /// Installed memory, rounded to the nearest GiB.
    pub ram_gib: u32,
    /// The renderer the host draws with (`software`, `gpu`, `none` for a
    /// headless run, `drivers` when the drivers draw).
    pub renderer: String,
    /// The CPU's model string, as the operating system reports it.
    pub cpu: String,
}

impl Environment {
    /// This computer, with `renderer`.
    pub fn detect(renderer: &str) -> Environment {
        let (os, os_version) = uname();
        Environment {
            os,
            os_version,
            arch: std::env::consts::ARCH.to_string(),
            cores: std::thread::available_parallelism().map_or(0, |n| n.get() as u32),
            ram_gib: ram_gib().unwrap_or(0),
            renderer: renderer.to_string(),
            cpu: cpu_model().unwrap_or_default(),
        }
    }
}

fn uname() -> (String, String) {
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

/// MemTotal, rounded to the nearest GiB (Linux).
fn ram_gib() -> Option<u32> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kib: u64 = text.lines().find_map(|l| l.strip_prefix("MemTotal:"))?.split_whitespace().next()?.parse().ok()?;
    Some(((kib + 512 * 1024) / (1024 * 1024)) as u32)
}

/// The first CPU's model name (Linux: `model name`, or `Hardware` /
/// `Processor` on some ARM systems).
fn cpu_model() -> Option<String> {
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

#[cfg(test)]
mod tests {
    #[test]
    fn detects_something_coarse() {
        let e = super::Environment::detect("none");
        assert!(!e.os.is_empty() && !e.arch.is_empty() && e.cores > 0, "{e:?}");
        assert_eq!(e.renderer, "none");
    }
}
