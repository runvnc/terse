//! Any other system: no readings. A host then blames by its waits alone
//! and never stops a device (`RECORD.md` section 4).

use crate::probe::ProcSample;

pub struct Probe;

impl Probe {
    pub fn process(_pid: u32) -> Probe {
        Probe
    }

    pub fn this_thread() -> Probe {
        Probe
    }

    pub fn has_cpu(&self) -> bool {
        false
    }

    pub fn has_wait(&self) -> bool {
        false
    }

    pub fn sample(&mut self) -> Option<ProcSample> {
        None
    }

    pub fn rss_kib(&self) -> Option<u64> {
        None
    }
}

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
    (std::env::consts::OS.to_string(), String::new())
}

pub fn ram_kib() -> Option<u64> {
    None
}

pub fn cpu_model() -> Option<String> {
    None
}
