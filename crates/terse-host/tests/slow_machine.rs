//! PERFORMANCE.md R15 in terse-host's window: the rule acts on what the
//! host times (R14). Each test runs `terse-host --selftest` in a headless
//! Wayland compositor of its own (weston's headless backend: no window is
//! ever shown), with the software renderer, scratch directories and no
//! relay, and reads the record. `TERSE_HOST_SLOW` makes one machine's
//! frames, or the drawing, sleep (blocked: not on a CPU, not waiting for
//! one). Skipped where there is no weston.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use terse_perflog::record::{self, Line};
use terse_perflog::{Component, Verdict};

/// The tests run one at a time: each times frames.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A headless weston, stopped when dropped.
struct Weston {
    child: Child,
    runtime: tempfile::TempDir,
    socket: String,
}

impl Drop for Weston {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Weston {
    fn start() -> Option<Weston> {
        let runtime = tempfile::tempdir().ok()?;
        std::fs::set_permissions(runtime.path(), std::os::unix::fs::PermissionsExt::from_mode(0o700)).ok()?;
        let socket = "terse-test".to_string();
        let child = Command::new("weston")
            .args(["--backend=headless", "--width=1024", "--height=768", "--idle-time=0"])
            .arg(format!("--socket={socket}"))
            .env("XDG_RUNTIME_DIR", runtime.path())
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let w = Weston { child, runtime, socket };
        let t = Instant::now();
        while !w.runtime.path().join(&w.socket).exists() {
            if t.elapsed() > Duration::from_secs(10) {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Some(w)
    }

    /// `terse-host` in this compositor, with scratch directories in `dir`,
    /// the software renderer, no relay, and the record in `dir/perf`.
    fn host(&self, dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
        self.host_with(dir, env!("CARGO_BIN_EXE_terse-host"), args, env)
    }

    /// [`Self::host`] through `program` (a wrapper that runs terse-host).
    fn host_with(&self, dir: &Path, program: &str, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut c = Command::new(program);
        for d in ["store", "config", "data", "perf"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        c.args(args)
            .args(["--renderer", "software", "--perf-rule", "on", "--perf-log"])
            .arg(dir.join("perf"))
            .arg("--store")
            .arg(dir.join("store"))
            .arg("--config")
            .arg(dir.join("config"))
            .arg("--data")
            .arg(dir.join("data"))
            .env("XDG_RUNTIME_DIR", self.runtime.path())
            .env("WAYLAND_DISPLAY", &self.socket)
            .env("XDG_CACHE_HOME", dir.join("cache"))
            .env("XDG_CONFIG_HOME", dir.join("xdg-config"))
            .env("XDG_DATA_HOME", dir.join("xdg-data"))
            .env("TERSE_AUDIO", "off")
            .env_remove("DISPLAY")
            .env_remove("TERSE_HOST_SLOW");
        for (k, v) in env {
            c.env(k, v);
        }
        c.output().unwrap()
    }
}

fn ready() -> Option<(Weston, tempfile::TempDir)> {
    let Some(w) = Weston::start() else {
        eprintln!("skipped: no headless weston");
        return None;
    };
    Some((w, tempfile::tempdir().unwrap()))
}

fn program(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../programs/{name}/{name}.terse")).display().to_string()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn record_lines(dir: &Path) -> Vec<Line> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir.join("perf")).unwrap().flatten() {
        if e.file_name().to_string_lossy().ends_with(".log") {
            out.extend(record::parse(&std::fs::read_to_string(e.path()).unwrap()));
        }
    }
    out
}

/// Whether the record says the frame loop's run-queue wait was read: the
/// blames are then certain, and the rule stops (`RECORD.md` 4).
fn certain(lines: &[Line]) -> bool {
    lines.iter().any(|l| matches!(l, Line::Readings { component: Component::Core, readings } if readings.wait))
}

/// Hello alone, its frames taking 40 ms more from the host's frame 40 on:
/// blamed (as `core`, hello's program named), warned about at the 3rd late
/// frame with its snapshot saved, saved again and halted at the 10th; the
/// run ends with status 4 naming the device, and the snapshot is a file in
/// the record's directory. Before the slow frames nothing was counted.
#[test]
fn a_slow_machine_alone_is_warned_about_then_saved_and_halted() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some((w, dir)) = ready() else { return };
    let hello = program("hello");
    let out = w.host(dir.path(), &["--selftest", &hello, "--frames", "200"], &[("TERSE_HOST_SLOW", "machine0:40:40")]);
    let err = text(&out.stderr);
    let lines = record_lines(dir.path());
    let blamed: Vec<u64> = lines
        .iter()
        .filter_map(|l| match l {
            Line::Late { frame, verdict: Verdict::Blamed, component: Component::Core, own_ms, image, .. } => {
                assert!(*own_ms >= 30.0 && image != "-" && !image.is_empty(), "{l:?}");
                Some(*frame)
            }
            _ => None,
        })
        .collect();
    assert!(blamed.first().is_some_and(|&f| f >= 40), "nothing before the slow frames: {lines:?}\n{err}");
    if !certain(&lines) {
        assert!(out.status.success(), "{err}");
        assert!(err.contains("is slow (3 late frames)") && !err.contains("stopped by the host"), "{err}");
        assert!(!lines.iter().any(|l| matches!(l, Line::Stop { .. })), "{lines:?}");
        return;
    }
    assert_eq!(out.status.code(), Some(4), "{err}");
    assert!(err.contains("is slow (3 late frames): your work is saved"), "{err}");
    assert!(err.contains("did not keep up (10 late frames): machine 0 stopped"), "{err}");
    assert!(err.contains("stopped by the host: device 1 did not keep up"), "{err}");
    assert!(lines.iter().any(|l| matches!(l, Line::Warn { component: Component::Core, count: 3, .. })), "{lines:?}");
    let stop = lines.iter().find_map(|l| if let Line::Stop { component, machine, saved, image, .. } = l { Some((*component, *machine, saved.clone(), image.clone())) } else { None });
    let (c, m, saved, image) = stop.expect("a stop line");
    assert_eq!((c, m, saved.as_str()), (Component::Core, 0, "file"));
    assert_eq!(blamed.len(), 10, "warned at the 3rd, stopped at the 10th: {lines:?}");
    let snap = std::fs::read_dir(dir.path().join("perf")).unwrap().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "tsnp"));
    let snap = snap.expect("the snapshot file");
    assert!(snap.file_name().unwrap().to_string_lossy().contains(&image), "{snap:?} {image}");
    // `terse perf log` names it.
    let summary = terse_perflog::summary::summarise(&dir.path().join("perf")).unwrap();
    assert!(summary.contains("stopped: frame") && summary.contains("core, machine 0"), "{summary}");
}

/// The shell with hello started (handle 1), hello's frames slow from the
/// host's frame 120 on: hello is saved into its automatic slot (16) at the
/// warning and halted at the stop; the shell goes on (the self-test ends
/// normally), and the slot is in the shell's snapshots on disk.
#[test]
fn a_slow_machine_of_the_shell_is_halted_not_the_shell() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some((w, dir)) = ready() else { return };
    let events = dir.path().join("start-hello.events");
    std::fs::write(&events, "2 press down\n3 press down\n4 press down\n5 press enter\n").unwrap();
    let events_s = events.display().to_string();
    let out = w.host(dir.path(), &["--selftest", "--shell", "--frames", "300", "--events", &events_s], &[("TERSE_HOST_SLOW", "machine1:40:120")]);
    let err = text(&out.stderr);
    let lines = record_lines(dir.path());
    assert!(out.status.success(), "the shell is not stopped: {err}");
    if !certain(&lines) {
        assert!(err.contains("machine 1 (“hello”) is slow"), "{err}");
        return;
    }
    assert!(err.contains("machine 1 (“hello”) is slow (3 late frames): your work is saved (snapshot slot 16)"), "{err}");
    assert!(err.contains("machine 1 (“hello”) did not keep up (10 late frames): machine 1 stopped; snapshot slot 16"), "{err}");
    let stops: Vec<(u32, String)> = lines.iter().filter_map(|l| if let Line::Stop { machine, saved, .. } = l { Some((*machine, saved.clone())) } else { None }).collect();
    assert_eq!(stops, vec![(1, "slot16".to_string())], "{lines:?}");
    // The slot, where the shell's snapshots are.
    let snapshots = dir.path().join("data");
    let found = walk(&snapshots).iter().any(|p| p.to_string_lossy().contains("16"));
    assert!(found, "{:?}", walk(&snapshots));
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p.clone());
            }
            out.push(p);
        }
    }
    out
}

/// The drawing takes 30 ms more from frame 40: the look is shed first,
/// then the host's own drawing is warned about, then, with nothing left
/// to shed, recorded as not keeping up (`fail`); no machine is stopped.
#[test]
fn slow_drawing_is_shed_then_warned_about_then_failed_and_no_machine_stopped() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some((w, dir)) = ready() else { return };
    let hello = program("hello");
    let out = w.host(dir.path(), &["--selftest", &hello, "--frames", "120"], &[("TERSE_HOST_SLOW", "compose:30:40")]);
    let err = text(&out.stderr);
    let lines = record_lines(dir.path());
    assert!(out.status.success(), "{err}");
    assert!(!lines.iter().any(|l| matches!(l, Line::Stop { .. })), "{lines:?}");
    assert!(lines.iter().any(|l| matches!(l, Line::Shed { why, .. } if why == "late")), "{lines:?}");
    assert!(lines.iter().any(|l| matches!(l, Line::Warn { component: Component::Compose, .. })), "{lines:?}");
    // The first counted blame comes after the look is shed to plain lines.
    let first_warn = lines.iter().position(|l| matches!(l, Line::Warn { .. })).unwrap();
    assert!(lines[..first_warn].iter().any(|l| matches!(l, Line::Shed { level: 3, .. })), "{lines:?}");
    if certain(&lines) {
        assert!(lines.iter().any(|l| l.as_fail().is_some_and(|f| f.1 == Component::Compose && f.2 == 10)), "{lines:?}");
        assert!(err.contains("does not keep up on this computer"), "{err}");
    }
}

/// The run's first frames are excused: a drawing slow from the first frame
/// on is recorded as `excused` (start) for them, never counted.
#[test]
fn the_first_frames_are_excused() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some((w, dir)) = ready() else { return };
    let hello = program("hello");
    let out = w.host(dir.path(), &["--selftest", &hello, "--frames", "25"], &[("TERSE_HOST_SLOW", "compose:30:0")]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let lines = record_lines(dir.path());
    // (A self-test of 25 frames presents most of them.)
    assert!(lines.iter().filter(|l| l.as_excused().is_some_and(|e| e.1 == "start")).count() >= 15, "{lines:?}");
    assert!(!lines.iter().any(|l| matches!(l, Line::Late { .. } | Line::Warn { .. })), "{lines:?}");
}

/// A busy CPU (`taskset`: on one CPU) shared with a CPU hog
/// that has the CPU's priority over the host (`nice`), and hello's frames
/// doing 5 ms of work on it from frame 40 (within the allowance of half a
/// frame): its frames are late, and the host waited for the CPU most of
/// that time: starved, nobody blamed, warned about or stopped.
#[test]
fn a_starved_machine_is_not_blamed() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !Path::new("/proc/self/schedstat").exists() || std::env::var("TERSE_PERF_PROBE").is_ok_and(|v| v == "none") {
        eprintln!("skipped: the run-queue wait cannot be read");
        return;
    }
    let cpu = "0";
    let Ok(mut hog) = Command::new("taskset").args(["-c", cpu, "sh", "-c", "while :; do :; done"]).stdout(Stdio::null()).spawn() else {
        eprintln!("skipped: no taskset");
        return;
    };
    let Some((w, dir)) = ready() else {
        let _ = hog.kill();
        return;
    };
    let hello = program("hello");
    let host = env!("CARGO_BIN_EXE_terse-host").to_string();
    let out = {
        // terse-host on the hog's CPU, at a lower priority.
        let mut args = vec!["-c", cpu, "nice", "-n", "5", host.as_str(), "--selftest", &hello, "--frames", "160"];
        args.extend(["--threads", "1"]);
        w.host_with(dir.path(), "taskset", &args, &[("TERSE_HOST_SLOW", "machine0:5:40:spin")])
    };
    let _ = hog.kill();
    let _ = hog.wait();
    let err = text(&out.stderr);
    assert!(out.status.success(), "{err}");
    let lines = record_lines(dir.path());
    let tripped: Vec<&Line> = lines.iter().filter(|l| matches!(l, Line::Warn { .. } | Line::Stop { .. } | Line::Late { verdict: Verdict::Blamed, .. }) || l.as_fail().is_some()).collect();
    assert!(tripped.is_empty(), "{tripped:?}\n{lines:?}");
    let starved = lines.iter().filter(|l| matches!(l, Line::Late { verdict: Verdict::Starved, .. })).count();
    assert!(starved >= 10, "the hog made the frames late: {lines:?}");
}

/// Every built-in program, alone and in the shell, runs 600 frames in the
/// window with the rule on and draws no warning, stop or failure: the rule
/// is not tripped by what Tersenet ships (on a computer that meets the
/// hardware floor and is not overloaded).
#[test]
fn the_built_in_programs_trip_nothing_in_600_frames() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some((w, dir)) = ready() else { return };
    let mut runs: Vec<(String, Vec<String>)> = ["reader", "sparks", "fetch", "hello", "sketch", "holo", "reports"]
        .iter()
        .map(|p| (p.to_string(), vec!["--selftest".to_string(), program(p), "--frames".into(), "600".into()]))
        .collect();
    runs.push(("shell".into(), vec!["--selftest".into(), "--shell".into(), "--frames".into(), "600".into()]));
    for (name, args) in runs {
        let d = dir.path().join(&name);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = w.host(&d, &args, &[]);
        let err = text(&out.stderr);
        assert!(out.status.success(), "{name}: {err}");
        let lines = record_lines(&d);
        let tripped: Vec<&Line> = lines.iter().filter(|l| matches!(l, Line::Warn { .. } | Line::Stop { .. }) || l.as_fail().is_some()).collect();
        let late = lines.iter().filter(|l| matches!(l, Line::Late { .. })).count();
        eprintln!("{name}: {late} late frames recorded, {} excused", lines.iter().filter(|l| l.as_excused().is_some()).count());
        assert!(tripped.is_empty(), "{name}: {tripped:?}\n{lines:?}");
    }
}
