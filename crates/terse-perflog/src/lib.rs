//! The performance record (`RECORD.md`) and the rule that makes a host's
//! own components accountable for late frames (`PERFORMANCE.md` R13–R16).
//!
//! - [`identity`]: who a host or a device implementation is (name,
//!   version, build), self-reported;
//! - [`env`]: a coarse description of the computer, nothing personal;
//! - [`probe`]: CPU time, run-queue wait and resident memory of a process,
//!   read from the operating system, so that time a component spent
//!   waiting for a CPU is told from time it spent working or blocked;
//! - [`judge`]: which component made a frame late, and what the host does
//!   about it (warn, stop);
//! - [`record`]: the record file's format, its writer (off the frame
//!   loop, bounded) and its parser;
//! - [`summary`]: `terse perf log`;
//! - [`report`]: the performance report a user may publish (`RECORD.md` section 5);
//! - [`aggregate`]: the verdicts of many published reports.
//!
//! Nothing here is machine state, and no machine can read any of it.

pub mod aggregate;
pub mod env;
pub mod identity;
pub mod judge;
pub mod probe;

/// What each operating system gives [`probe`] and [`env`]: one file per
/// system, the same interface.
#[cfg_attr(target_os = "linux", path = "os/linux.rs")]
#[cfg_attr(target_os = "macos", path = "os/macos.rs")]
#[cfg_attr(windows, path = "os/windows.rs")]
#[cfg_attr(not(any(target_os = "linux", target_os = "macos", windows)), path = "os/other.rs")]
mod os;
pub mod record;
pub mod report;
pub mod summary;

pub use identity::Identity;
pub use judge::{Action, Component, FrameSample, Judge, Late, Part, Rule, Step, Tally, Verdict};
pub use record::{Line, Recorder};
