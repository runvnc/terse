//! A coarse description of the computer a record was made on (`RECORD.md`
//! `env`): enough to compare like with like, nothing that names the
//! machine or its user. No host name, user name, serial number, network
//! address or path; the RAM is rounded to whole GiB.

use crate::os;

/// The computer, as the record's `env` line gives it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Environment {
    /// The operating system's name (`Linux`, `Darwin`, `Windows`) and
    /// release (the kernel's; Windows: major.minor.build).
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
        let (os, os_version) = os::os_name();
        Environment {
            os,
            os_version,
            arch: std::env::consts::ARCH.to_string(),
            cores: std::thread::available_parallelism().map_or(0, |n| n.get() as u32),
            ram_gib: os::ram_kib().map_or(0, |kib| ((kib + 512 * 1024) / (1024 * 1024)) as u32),
            renderer: renderer.to_string(),
            cpu: os::cpu_model().unwrap_or_default(),
        }
    }
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
