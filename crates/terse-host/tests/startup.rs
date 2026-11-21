//! PERFORMANCE.md R13, SPEC §responsiveness item 6: before its first frame
//! a host reads only the image it runs, its settings, the fonts they list
//! and its own store and data directories. It waits on no relay and looks
//! in no directory of fonts, so the first frame depends on those files
//! alone, not on the network or on what fonts the system has.

#![cfg(unix)]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A child process killed and reaped when dropped.
struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn fifo(path: &Path) {
    let ok = Command::new("mkfifo").arg(path).status().expect("mkfifo").success();
    assert!(ok, "mkfifo {}", path.display());
}

/// Everything under `dir`, as paths relative to it.
fn tree(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            out.push(p.strip_prefix(dir).unwrap().display().to_string());
            if p.is_dir() && !p.is_symlink() {
                stack.push(p);
            }
        }
    }
    out.sort();
    out
}

/// The built-in shell's first frame, with a relay that accepts connections
/// and never answers (a host waiting for it would wait 10 s, its timeout)
/// and font directories whose only "fonts" are FIFOs (opening one blocks
/// until something writes to it, so a host that looked through them would
/// never draw at all): one extra font listed in the settings, in a directory
/// beside such a FIFO; the user's font directories under a new HOME and
/// XDG_DATA_HOME; a fontconfig file naming another. The first frame comes at
/// once, and nothing is written outside the store and the data directory.
#[test]
fn the_first_frame_waits_on_nothing_but_the_image_settings_and_listed_fonts() {
    let tmp = tempfile::tempdir().unwrap();
    let at = |s: &str| tmp.path().join(s);
    let dirs = ["home/.fonts", "home/.local/share/fonts", "xdg-data/fonts", "xdg-dirs/fonts", "xdg-config", "xdg-cache", "run", "listed", "config", "store", "data"];
    for d in dirs {
        std::fs::create_dir_all(at(d)).unwrap();
    }
    std::fs::set_permissions(at("run"), std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    for f in ["home/.fonts/a.ttf", "home/.local/share/fonts/a.ttf", "xdg-data/fonts/a.ttf", "xdg-dirs/fonts/a.ttf", "listed/other.ttf"] {
        fifo(&at(f));
    }
    std::fs::write(at("fonts.conf"), format!("<?xml version=\"1.0\"?>\n<fontconfig><dir>{}</dir></fontconfig>\n", at("listed").display()))
        .unwrap();
    let extra = at("listed/extra.ttf");
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/fonts/DejaVuSans.ttf"), &extra).unwrap();
    std::fs::write(at("config/fonts"), format!("extra {}\n", extra.display())).unwrap();
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let relay = format!("ws://{}", silent.local_addr().unwrap());
    let before = tree(tmp.path());

    let started = Instant::now();
    let child = Command::new(env!("CARGO_BIN_EXE_terse-host"))
        .args(["--bench-render", "--renderer", "software", "--warmup", "0", "--frames", "1", "--relay", &relay, "--store"])
        .arg(at("store"))
        .arg("--config")
        .arg(at("config"))
        .arg("--data")
        .arg(at("data"))
        .env("HOME", at("home"))
        .env("XDG_CONFIG_HOME", at("xdg-config"))
        .env("XDG_CACHE_HOME", at("xdg-cache"))
        .env("XDG_DATA_HOME", at("xdg-data"))
        .env("XDG_DATA_DIRS", at("xdg-dirs"))
        .env("XDG_RUNTIME_DIR", at("run"))
        .env("FONTCONFIG_FILE", at("fonts.conf"))
        .env_remove("TERSE_NODE_SOCKET")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = Killed(child);
    let status = loop {
        if let Some(s) = child.0.try_wait().unwrap() {
            break s;
        }
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "no first frame after 8 s: the host waited on the relay or opened a font it was not given"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut out = String::new();
    std::io::Read::read_to_string(child.0.stdout.as_mut().unwrap(), &mut out).unwrap();
    assert!(status.success(), "{status}: {out}");
    assert!(out.contains("1 frames"), "{out}");

    // Written: the store (the built-in programs and the manual) and the
    // data directory; nothing else.
    let after: Vec<String> =
        tree(tmp.path()).into_iter().filter(|p| !p.starts_with("store/") && !p.starts_with("data/") && !before.contains(p)).collect();
    assert_eq!(after, Vec::<String>::new());
    assert!(!tree(&at("store")).is_empty(), "the built-in programs are put in the store");
}
