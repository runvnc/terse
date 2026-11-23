//! Sound that cannot be heard is said (spec/sound.md §sound.presentation),
//! in terse-host's window: `programs/tune` under `--selftest` in a headless
//! weston of its own, with `TERSE_AUDIO=none` (as if no output were found:
//! nothing is ever opened, so no test makes a sound). Skipped where there
//! is no weston.

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Weston {
    child: Child,
    runtime: tempfile::TempDir,
}

impl Drop for Weston {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const SOCKET: &str = "terse-sound-test";

fn weston() -> Option<Weston> {
    let runtime = tempfile::tempdir().ok()?;
    std::fs::set_permissions(runtime.path(), std::os::unix::fs::PermissionsExt::from_mode(0o700)).ok()?;
    let child = Command::new("weston")
        .args(["--backend=headless", "--width=1024", "--height=768", "--idle-time=0"])
        .arg(format!("--socket={SOCKET}"))
        .env("XDG_RUNTIME_DIR", runtime.path())
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let w = Weston { child, runtime };
    let t = Instant::now();
    while !w.runtime.path().join(SOCKET).exists() {
        if t.elapsed() > Duration::from_secs(10) {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Some(w)
}

fn tune(w: &Weston, dir: &std::path::Path, audio: &str, frames: &str) -> Output {
    let tune = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../programs/tune/tune.terse");
    let mut c = Command::new(env!("CARGO_BIN_EXE_terse-host"));
    for d in ["store", "config", "data"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    c.arg("--selftest")
        .arg(&tune)
        .args(["--frames", frames, "--renderer", "software", "--perf-log", "off", "--store"])
        .arg(dir.join("store"))
        .arg("--config")
        .arg(dir.join("config"))
        .arg("--data")
        .arg(dir.join("data"))
        .env("XDG_RUNTIME_DIR", w.runtime.path())
        .env("WAYLAND_DISPLAY", SOCKET)
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_CONFIG_HOME", dir.join("xdg-config"))
        .env("XDG_DATA_HOME", dir.join("xdg-data"))
        .env("TERSE_AUDIO", audio)
        .env_remove("DISPLAY")
        .env_remove("TERSE_HOST_SLOW");
    c.output().unwrap()
}

/// `--sound-check` (PERFORMANCE.md R19): tune's frames, headless, handed
/// to the null output, with no audio device and no window. Every frame
/// with sound is handed over and the line says how long that took; the
/// times themselves are `terse perf`'s (S1) to judge, not a test's.
#[test]
fn sound_check_runs_headless_with_no_device() {
    let dir = tempfile::tempdir().unwrap();
    for d in ["store", "config", "data"] {
        std::fs::create_dir_all(dir.path().join(d)).unwrap();
    }
    let tune = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../programs/tune/tune.terse");
    let o = Command::new(env!("CARGO_BIN_EXE_terse-host"))
        .arg(&tune)
        .args(["--sound-check", "--frames", "90", "--store"])
        .arg(dir.path().join("store"))
        .arg("--config")
        .arg(dir.path().join("config"))
        .arg("--data")
        .arg(dir.path().join("data"))
        .env("XDG_CACHE_HOME", dir.path().join("cache"))
        .env("XDG_CONFIG_HOME", dir.path().join("xdg-config"))
        .env("XDG_DATA_HOME", dir.path().join("xdg-data"))
        // No sound server and no ALSA device: nothing it could reach.
        .env("TERSE_AUDIO", "none")
        .env("PULSE_SERVER", "unix:/nonexistent")
        .env("ALSA_CONFIG_PATH", "/nonexistent")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .output()
        .unwrap();
    let (out, err) = (String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(o.status.success(), "{out}{err}");
    assert!(!err.contains("no sound"), "{err}");
    let line = out.lines().find(|l| l.starts_with("sound-check: ")).unwrap_or_else(|| panic!("{out}{err}"));
    assert!(line.contains("the null output"), "{line}");
    let after = |key: &str| -> f64 { line.split(key).nth(1).and_then(|s| s.split([' ', ',', ';']).next()?.parse().ok()).unwrap_or_else(|| panic!("{key}: {line}")) };
    let (handed, open) = (after("sound-check: "), after(") ("));
    assert!(handed >= 1.0 && handed <= open, "{line}");
    if after("pairs dropped ") == 0.0 {
        assert_eq!(handed, open, "every frame with the output open is handed over: {line}");
    }
    for key in [" p50 ", " p99 ", " max "] {
        assert!(after(key) >= 0.0, "{line}");
    }
    assert!(after(" max ") >= after(" p50 "), "{line}");
    assert_eq!(after("output buffer "), 50.0, "three frames: {line}");
    assert!(line.contains("underruns "), "{line}");
}

/// The volume keys change the volume for this run only: nothing is
/// written to the config directory, and a `sound` file an earlier host
/// wrote there is left as it was (and not read).
#[test]
fn the_volume_is_not_saved() {
    if !Command::new("weston").arg("--version").output().is_ok_and(|o| o.status.success()) {
        terse_testkit::skip!("weston", "no weston: --drive needs a headless compositor");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let hello = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../programs/hello/hello.terse");
    let script = dir.path().join("script.txt");
    std::fs::write(&script, "wait 5\npress 0x2D ctrl alt\nwait 2\npress m ctrl alt\nwait 2\npress 0x2E ctrl alt shift\nwait 2\nquit\n").unwrap();
    for (case, before) in [("none", None), ("earlier", Some("volume 30\nmute on\n"))] {
        let d = dir.path().join(case);
        for sub in ["store", "config", "data"] {
            std::fs::create_dir_all(d.join(sub)).unwrap();
        }
        if let Some(text) = before {
            std::fs::write(d.join("config/sound"), text).unwrap();
        }
        let o = Command::new(env!("CARGO_BIN_EXE_terse-host"))
            .arg(&hello)
            .arg("--drive")
            .arg(&script)
            .args(["--renderer", "software", "--consent", "deny", "--perf-log", "off", "--volume", "70", "--store"])
            .arg(d.join("store"))
            .arg("--config")
            .arg(d.join("config"))
            .arg("--data")
            .arg(d.join("data"))
            .env("XDG_CACHE_HOME", d.join("cache"))
            .env("XDG_CONFIG_HOME", d.join("xdg-config"))
            .env("XDG_DATA_HOME", d.join("xdg-data"))
            .env("TERSE_AUDIO", "none")
            .env_remove("TERSE_DRIVE_DISPLAY")
            .env_remove("TERSE_DRIVE_NESTED")
            .output()
            .unwrap();
        let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        assert!(o.status.success(), "{text}");
        assert_eq!(std::fs::read_to_string(d.join("config/sound")).ok().as_deref(), before, "{case}: {text}");
        for sub in ["config", "xdg-config"] {
            let found: Vec<_> = walk(&d.join(sub)).into_iter().filter(|p| p.file_name().is_some_and(|n| n == "sound")).collect();
            assert!(found.len() == usize::from(before.is_some() && sub == "config"), "{case}: {found:?}");
        }
    }
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// No output: one line on standard error, once, with the reason; the
/// machine runs as usual, as with sound off, which opens nothing and says
/// nothing. (That the state never depends on the output holds by
/// construction: the host's sound only reads the mixed samples.)
#[test]
fn no_output_is_said_once_and_changes_nothing() {
    let Some(w) = weston() else {
        terse_testkit::skip!("weston", "no headless weston");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let none = tune(&w, &dir.path().join("none"), "none", "240");
    let err = String::from_utf8_lossy(&none.stderr);
    assert!(none.status.success(), "{err}");
    assert_eq!(err.matches("no sound:").count(), 1, "{err}");
    assert!(err.contains("terse-host: no sound: no audio output was found (TERSE_AUDIO=none)"), "{err}");
    let off = tune(&w, &dir.path().join("off"), "off", "240");
    let off_err = String::from_utf8_lossy(&off.stderr);
    assert!(off.status.success(), "{off_err}");
    assert!(!off_err.contains("no sound"), "{off_err}");
    for o in [&none, &off] {
        let out = String::from_utf8_lossy(&o.stdout);
        assert!(out.contains("selftest: 240 machine frames"), "{out}");
    }
}
