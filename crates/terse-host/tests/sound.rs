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
