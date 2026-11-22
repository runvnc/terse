//! The terminal's cursor (SPEC §terminal.presentation): a host draws it
//! only once the program has used the terminal, so a program that never
//! touches it shows no cursor, in either renderer and in the phosphor look
//! (its stroke text draws the cursor by the beam). Driven in the real
//! window (`--drive`, in its own headless compositor; skipped where there
//! is none).

#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Command, Output};

use terse_core::image::DeviceReq;
use terse_core::worst::{Builder, Call};

/// The tests run one at a time: each runs a window in real time.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn have_weston() -> bool {
    let ok = Command::new("weston").arg("--version").output().is_ok_and(|o| o.status.success());
    if !ok {
        terse_testkit::skip!("weston", "no weston: --drive needs a headless compositor");
    }
    ok
}

fn text(o: &Output) -> String {
    format!("status {:?}\n--- stdout\n{}--- stderr\n{}", o.status, String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

/// An image with the terminal that makes `calls` in its first frame, then
/// nothing.
fn image(dir: &Path, name: &str, calls: Vec<Call>) -> std::path::PathBuf {
    let mut b = Builder::new();
    b.frame(calls);
    b.frame(Vec::new());
    let path = dir.join(format!("{name}.terse"));
    std::fs::write(&path, b.build(name, &[DeviceReq::new(2, 0)])).unwrap();
    path
}

/// The brightest pixel of cell (0, 0) (11 × 22 window pixels at 880 × 660)
/// in the picture `shot`.
fn cell_light(shot: &Path) -> u8 {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(shot).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    let channels = info.color_type.samples();
    let mut max = 0;
    for y in 0..22 {
        for x in 0..11 {
            let at = (y * info.width as usize + x) * channels;
            max = max.max(buf[at]).max(buf[at + 1]).max(buf[at + 2]);
        }
    }
    max
}

#[test]
fn no_cursor_for_an_unused_terminal() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // Unused: no call. Used: set_cursor(0, 0) leaves the state as reset
    // left it, but the program has used the terminal.
    let unused = image(dir.path(), "unused", Vec::new());
    let used = image(dir.path(), "used", vec![Call::new(0x202, [0, 0, 0, 0])]);
    for renderer in ["software", "gpu"] {
        for look in [None, Some("--force-phosphor")] {
            let mut light = Vec::new();
            for img in [&unused, &used] {
                let shot = dir.path().join("shot.png");
                let script = dir.path().join("script.txt");
                std::fs::write(&script, format!("wait 30\nshot {}\nquit\n", shot.display())).unwrap();
                let o = Command::new(env!("CARGO_BIN_EXE_terse-host"))
                    .arg(img)
                    .arg("--drive")
                    .arg(&script)
                    // The rule's slow-drawing notice, a terminal of the host's
                    // over the bottom rows, would take the cursor's place on a
                    // loaded machine: this test is about the cursor alone.
                    .args(["--renderer", renderer, "--size", "880x660", "--mute", "--perf-rule", "off"])
                    .args(look)
                    .env("XDG_CACHE_HOME", dir.path().join("cache"))
                    .env("XDG_CONFIG_HOME", dir.path().join("xdg-config"))
                    .env("XDG_DATA_HOME", dir.path().join("xdg-data"))
                    .env_remove("TERSE_DRIVE_DISPLAY")
                    .env_remove("TERSE_DRIVE_NESTED")
                    .current_dir(dir.path())
                    .output()
                    .unwrap();
                assert!(o.status.success(), "{renderer} {look:?}: {}", text(&o));
                light.push(cell_light(&shot));
                // TERSE_TEST_KEEP_SHOT=<dir>: the pictures, to look at.
                if let Some(keep) = std::env::var_os("TERSE_TEST_KEEP_SHOT") {
                    let name = format!("{}-{renderer}-{}.png", img.file_stem().unwrap().to_string_lossy(), look.is_some());
                    std::fs::copy(&shot, Path::new(&keep).join(name)).unwrap();
                }
            }
            let what = format!("{renderer} {look:?}: cell (0, 0) at most {:?} (unused, used)", light);
            assert!(light[0] < 40, "no cursor for the unused terminal: {what}");
            assert!(light[1] > light[0] + 60, "the cursor once used: {what}");
        }
    }
}
