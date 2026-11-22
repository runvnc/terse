//! A driver that does not keep up (`PERFORMANCE.md` R14, R15; `RECORD.md`):
//! `native/driver-example`'s deliberately slow mode under terse-bus. A
//! driver that blocks or spins in END is blamed, warned about at its 3rd
//! late frame and has its machine snapshotted and stopped at its 10th
//! (exit status 4); the snapshot restores and goes on exactly as the
//! machine would have with a healthy driver; a replay of the run's
//! recording stops where the run did; the record says all of it. A driver
//! that is merely starved (a CPU hog on its CPU, the driver at the lowest
//! priority) makes frames late but is never blamed.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};

use terse_perflog::record::{self, Line};
use terse_perflog::{Component, Verdict};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// terse-bus (another package's program), built from the current sources,
/// beside this package's drivers.
fn terse_bus() -> PathBuf {
    terse_testkit::program("terse-bus")
}

fn hello() -> String {
    workspace().join("programs/hello/hello.terse").display().to_string()
}

/// The example driver, built here (`None`: no C compiler).
fn example(dir: &Path) -> Option<String> {
    let out = dir.join("terminal");
    let src = workspace().join("native/driver-example/terminal.c");
    let ok = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()))
        .args(["-std=c11", "-O2", "-o"])
        .arg(&out)
        .arg(&src)
        .status()
        .is_ok_and(|s| s.success());
    ok.then(|| out.display().to_string())
}

fn bus(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = Command::new(terse_bus());
    c.args(args).env_remove("TERSE_EXAMPLE_SLOW").env_remove("TERSE_EXAMPLE_IDLE").env_remove("TERSE_BUS_CPU");
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Every record line in `dir`.
fn record_lines(dir: &Path) -> Vec<Line> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        if e.file_name().to_string_lossy().ends_with(".log") {
            out.extend(record::parse(&std::fs::read_to_string(e.path()).unwrap()));
        }
    }
    out
}

/// The tests run one at a time: the starved one's CPU hog would starve the
/// others' drivers too (and rightly excuse their late frames).
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn ready() -> Option<(tempfile::TempDir, String)> {
    terse_bus();
    let tmp = tempfile::tempdir().unwrap();
    match example(tmp.path()) {
        Some(x) => Some((tmp, x)),
        None => {
            terse_testkit::skip!("cc", "no C compiler for native/driver-example");
            None
        }
    }
}

/// A driver that sleeps 60 ms in END from the END of frame 19 on (END
/// carries the counter after the frame, 20; blocked: not on a CPU, not
/// waiting for one). The run records every frame's hash (--record), which
/// asks the driver for its state right after each frame: that waits for
/// END's work, in the same frame, and counts as the driver's. So it is
/// blamed from frame 19 on: on an idle computer in frames 19, 20, ...,
/// warned about in frame 21 (3 within 60: the machine is saved at once)
/// and its machine saved and stopped in frame 28 (10 within 300). On a
/// busy one a frame may be excused as starved, and the warning and the
/// stop come a frame later: the test checks the rule, not the computer.
#[test]
fn a_blocked_driver_is_warned_about_then_its_machine_saved_and_stopped() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some((tmp, x)) = ready() else { return };
    let perf = tmp.path().join("perf");
    let rec = tmp.path().join("run.tbrc");
    let driver = format!("2={x}");
    let (perf_s, rec_s) = (perf.display().to_string(), rec.display().to_string());
    let out = bus(
        &[&hello(), "--frames", "300", "--hash", "--driver", &driver, "--perf-log", &perf_s, "--perf-rule", "on", "--record", &rec_s],
        &[("TERSE_EXAMPLE_SLOW", "sleep:60:20")],
    );
    let err = text(&out.stderr);
    // Where the system does not say how long the driver and the frame
    // loop waited for a CPU (Windows; TERSE_PERF_PROBE=none), the driver
    // is blamed by its waits alone: warned about, never stopped
    // (RECORD.md 4). The record's readings lines say which case this is.
    let lines = record_lines(&perf);
    let waits = |c: Component| lines.iter().any(|l| matches!(l, Line::Readings { component, readings } if *component == c && readings.wait));
    if !(waits(Component::Core) && waits(Component::Device(2))) {
        assert!(out.status.success(), "{err}");
        assert!(err.contains("is slow (3 late frames)") && !err.contains("stopped by the host"), "{err}");
        assert!(lines.iter().any(|l| matches!(l, Line::Warn { component: Component::Device(2), .. })), "{lines:?}");
        assert!(!lines.iter().any(|l| matches!(l, Line::Stop { .. })), "{lines:?}");
        let blamed = lines.iter().filter(|l| matches!(l, Line::Late { verdict: Verdict::Blamed, component: Component::Device(2), .. })).count();
        assert!(blamed >= 10, "{lines:?}");
        let summary = terse_perflog::summary::summarise(&perf).unwrap();
        assert!(summary.contains("no CPU-wait readings for dev2"), "{summary}");
        return;
    }
    assert_eq!(out.status.code(), Some(4), "{err}");
    assert!(err.contains("device 2 (terminal (example) 0.1.0 fnv:") && err.contains("is slow (3 late frames)"), "{err}");
    assert!(err.contains("did not keep up (10 late frames)") && err.contains("stopped by the host: device 2"), "{err}");
    let halted_hash = text(&out.stdout).trim().to_string();
    assert_eq!(halted_hash.len(), 64);

    // The record: who, what was late and why, the warning, the stop.
    assert!(lines.contains(&Line::Format(1)));
    assert!(lines.iter().any(|l| matches!(l, Line::Host(i) if i.name == "terse-bus" && i.build.starts_with("b3:"))), "{lines:?}");
    assert!(lines.iter().any(|l| matches!(l, Line::Device { component: Component::Device(2), identity, .. } if identity.name == "terminal (example)" && identity.build.starts_with("fnv:"))));
    assert!(lines.iter().any(|l| matches!(l, Line::Env(e) if e.cores > 0)));
    assert!(lines.iter().any(|l| matches!(l, Line::Start { .. })));
    let blamed: Vec<u64> = lines
        .iter()
        .filter_map(|l| match l {
            Line::Late { frame, verdict: Verdict::Blamed, component: Component::Device(2), blocked_ms, own_ms, .. } => {
                assert!(*own_ms >= 40.0 && *blocked_ms >= 30.0, "{l:?}");
                Some(*frame)
            }
            _ => None,
        })
        .collect();
    assert!(blamed.len() == 10 && blamed[0] >= 19, "nothing before the slow frames, 10 to the stop: {lines:?}");
    // Every frame from the first slow one to the stop was late: blamed,
    // or excused because the computer was busy.
    let late = lines.iter().filter(|l| matches!(l, Line::Late { frame, component: Component::Device(2), .. } if *frame >= 19)).count() as u64;
    let (warn_at, stop_at) = (blamed[2], blamed[9]);
    assert_eq!(late, stop_at - 18, "{lines:?}");
    // The stop comes with the tenth blamed frame: frame 28 on a quiet
    // computer. On a busy one any number of the frames between are
    // excused ("memory", "starved") and the stop comes that much later;
    // the count above already checks that each of them was late, so no
    // upper limit is set here beyond the run's 300 frames.
    assert!(stop_at >= 28, "{lines:?}");
    assert!(lines.contains(&Line::Warn { frame: warn_at, component: Component::Device(2), count: 3 }), "{lines:?}");
    let stop = lines.iter().find_map(|l| if let Line::Stop { frame, component, machine, saved, .. } = l { Some((*frame, *component, *machine, saved.clone())) } else { None });
    assert_eq!(stop, Some((stop_at, Component::Device(2), 0, "file".to_string())));
    assert!(lines.iter().any(|l| matches!(l, Line::End { frames, late, .. } if *frames == stop_at + 1 && *late >= 10)), "{lines:?}");

    // The snapshot restores and goes on as the machine would have with a
    // healthy driver: ten more frames give the hash of as many uninterrupted.
    let snap = std::fs::read_dir(&perf).unwrap().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "tsnp")).expect("the snapshot file");
    let snap_s = snap.display().to_string();
    let total = stop_at + 11;
    let whole = bus(&[&hello(), "--frames", &total.to_string(), "--hash"], &[]);
    let rest = (total - (stop_at + 1)).to_string();
    let resumed = bus(&[&hello(), "--load-state", &snap_s, "--frames", &rest, "--hash"], &[]);
    assert!(whole.status.success() && resumed.status.success(), "{}", text(&resumed.stderr));
    assert_eq!(text(&resumed.stdout), text(&whole.stdout));
    // (The example driver leaves RESTORE out, DEVICES.md 13: the healthy
    // driver here is the reference terminal driver.)

    // A replay of the recording stops where the run did, with its hash,
    // with no slow driver and no rule.
    let replay = bus(&[&hello(), "--replay", &rec_s, "--hash"], &[]);
    let rerr = text(&replay.stderr);
    assert_eq!(replay.status.code(), Some(4), "{rerr}");
    assert!(!rerr.contains("differs"), "{rerr}");
    assert_eq!(text(&replay.stdout).trim(), halted_hash);

    // `terse perf log`'s summary of the record.
    let summary = terse_perflog::summary::summarise(&perf).unwrap();
    assert!(
        summary.contains("dev2: blamed 10") && summary.contains(&format!("warning: frame {warn_at}: dev2 late 3 times")) && summary.contains(&format!("stopped: frame {stop_at}: dev2")),
        "{summary}"
    );
}

/// A driver that spins 40 ms in END (on a CPU): blamed with its time on
/// the CPU; without the rule it is only recorded.
#[test]
fn a_spinning_driver_is_blamed_and_without_the_rule_only_recorded() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some((tmp, x)) = ready() else { return };
    let perf = tmp.path().join("perf");
    let driver = format!("2={x}");
    let perf_s = perf.display().to_string();
    let out = bus(&[&hello(), "--frames", "40", "--hash", "--driver", &driver, "--perf-log", &perf_s], &[("TERSE_EXAMPLE_SLOW", "spin:40:20:35")]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let healthy = bus(&[&hello(), "--frames", "40", "--hash"], &[]);
    assert_eq!(text(&out.stdout), text(&healthy.stdout), "a slow driver changes nothing in the machine");
    let lines = record_lines(&perf);
    let late: Vec<&Line> = lines.iter().filter(|l| matches!(l, Line::Late { component: Component::Device(2), .. })).collect();
    let blamed = late.iter().filter(|l| matches!(l, Line::Late { verdict: Verdict::Blamed, cpu_ms, .. } if *cpu_ms >= 15.0)).count();
    // The machine may well share its CPU with others; at least most of
    // the 15 slow frames are its own CPU time.
    assert!(blamed >= 10, "{late:?}");
    assert!(!lines.iter().any(|l| matches!(l, Line::Warn { .. } | Line::Stop { .. })), "the rule is off headless: {lines:?}");
}

/// A CPU hog on one CPU, stopped when dropped.
#[cfg(target_os = "linux")]
struct Hog {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(target_os = "linux")]
impl Hog {
    fn on(cpu: usize) -> Hog {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::spawn(move || {
            pin(cpu);
            let mut x = 0u64;
            while !s.load(Ordering::Relaxed) {
                x = std::hint::black_box(x.wrapping_add(1));
            }
        });
        Hog { stop, thread: Some(thread) }
    }
}

#[cfg(target_os = "linux")]
impl Drop for Hog {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(target_os = "linux")]
fn pin(cpu: usize) {
    // SAFETY: a zeroed cpu_set_t with one CPU set, for this thread.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_SET(cpu, &mut set);
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
    }
}

/// The last CPU this process may run on.
#[cfg(target_os = "linux")]
fn a_cpu() -> Option<usize> {
    // SAFETY: reads this process's affinity mask.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) != 0 {
            return None;
        }
        (0..libc::CPU_SETSIZE as usize).rev().find(|&c| libc::CPU_ISSET(c, &set))
    }
}

/// The computer is busy: the driver runs at the lowest priority
/// (SCHED_IDLE) on a CPU a hog keeps busy, and does 5 ms of work in END
/// (well within its allowance of half a frame). Its frames are late and
/// the core waits for it longer than its allowance, but it spent that
/// time waiting for the CPU: it is never blamed, warned about or stopped.
/// Linux only: it needs CPU affinity and SCHED_IDLE to make the case.
#[cfg(target_os = "linux")]
#[test]
fn a_starved_driver_is_not_blamed() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some((tmp, x)) = ready() else { return };
    if terse_perflog::probe::disabled() {
        terse_testkit::skip!("probe", "TERSE_PERF_PROBE=none (starvation cannot be read, so it is blamed)");
        return;
    }
    if !Path::new("/proc/self/schedstat").exists() {
        terse_testkit::skip!("schedstat", "no /proc/self/schedstat (the run-queue wait cannot be read)");
        return;
    }
    let Some(cpu) = a_cpu() else { return };
    let perf = tmp.path().join("perf");
    let driver = format!("2={x}");
    let perf_s = perf.display().to_string();
    let cpu_s = cpu.to_string();
    let _hog = Hog::on(cpu);
    let out = bus(
        &[&hello(), "--frames", "40", "--hash", "--driver", &driver, "--perf-log", &perf_s, "--perf-rule", "on"],
        &[("TERSE_EXAMPLE_SLOW", "cpu:5:5"), ("TERSE_EXAMPLE_IDLE", "1"), ("TERSE_BUS_CPU", &cpu_s)],
    );
    drop(_hog);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let lines = record_lines(&perf);
    assert!(!lines.iter().any(|l| matches!(l, Line::Warn { .. } | Line::Stop { .. })), "{lines:?}");
    assert!(!lines.iter().any(|l| matches!(l, Line::Late { verdict: Verdict::Blamed, .. })), "{lines:?}");
    // The evidence: late frames in which the core waited for the driver
    // longer than the allowance (8.3 ms), most of it starved.
    let starved = lines
        .iter()
        .filter(|l| matches!(l, Line::Late { verdict: Verdict::Starved, component: Component::Device(2), own_ms, starved_ms, .. } if *own_ms + *starved_ms > 8.4 && *starved_ms > *own_ms))
        .count();
    assert!(starved >= 10, "the hog made the driver's frames late: {lines:?}");
}
