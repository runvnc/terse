//! PERFORMANCE.md R17, SPEC §responsiveness item 6: before its first frame
//! a host reads only the image it runs, its settings, the fonts they list
//! and its own store and data directories. It waits on no relay and looks
//! in no directory of fonts, so the first frame depends on those files
//! alone, not on the network or on what fonts the system has.
//!
//! What it proves is said by order and by what the host did, not by how
//! long it took (a busy computer may take any time to start): no FIFO
//! among the fonts was ever opened (a reader's open shows to a writer's),
//! and no connection to the relay was given up before the first frame was
//! done (a host that waited on this relay, which never answers, gives up
//! at its own timeout and closes the connection). The only time here
//! bounds a hang.

#![cfg(unix)]

use std::io::{BufRead, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// A child process killed and reaped when dropped.
struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `O_NONBLOCK` (not in std).
#[cfg(target_os = "linux")]
const O_NONBLOCK: i32 = 0o4000;
#[cfg(not(target_os = "linux"))]
const O_NONBLOCK: i32 = 0x0004;

/// Whether someone has the FIFO open for reading (or is blocked opening
/// it): opening its write end without waiting succeeds only then (ENXIO
/// otherwise). Opening it also lets such a reader go on.
fn fifo_has_reader(path: &Path) -> bool {
    std::fs::OpenOptions::new().write(true).custom_flags(O_NONBLOCK).open(path).is_ok()
}

/// A relay that accepts connections and never answers, counting those
/// that the other side closed.
fn silent_relay() -> (String, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let closed = Arc::new(AtomicUsize::new(0));
    let c = closed.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let c = c.clone();
            std::thread::spawn(move || {
                let mut conn = conn;
                let mut buf = [0u8; 4096];
                while matches!(conn.read(&mut buf), Ok(n) if n > 0) {}
                c.fetch_add(1, Ordering::SeqCst);
            });
        }
    });
    (url, closed)
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
/// XDG_DATA_HOME; a fontconfig file naming another. The first frame comes
/// with no FIFO opened and no relay connection given up before it, and
/// nothing is written outside the store and the data directory.
#[test]
fn the_first_frame_waits_on_nothing_but_the_image_settings_and_listed_fonts() {
    let tmp = tempfile::tempdir().unwrap();
    let at = |s: &str| tmp.path().join(s);
    let dirs = ["home/.fonts", "home/.local/share/fonts", "xdg-data/fonts", "xdg-dirs/fonts", "xdg-config", "xdg-cache", "run", "listed", "config", "store", "data"];
    for d in dirs {
        std::fs::create_dir_all(at(d)).unwrap();
    }
    std::fs::set_permissions(at("run"), std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let fifos: Vec<PathBuf> = ["home/.fonts/a.ttf", "home/.local/share/fonts/a.ttf", "xdg-data/fonts/a.ttf", "xdg-dirs/fonts/a.ttf", "listed/other.ttf"].iter().map(|f| at(f)).collect();
    for f in &fifos {
        fifo(f);
    }
    std::fs::write(at("fonts.conf"), format!("<?xml version=\"1.0\"?>\n<fontconfig><dir>{}</dir></fontconfig>\n", at("listed").display()))
        .unwrap();
    let extra = at("listed/extra.ttf");
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/fonts/DejaVuSans.ttf"), &extra).unwrap();
    std::fs::write(at("config/fonts"), format!("extra {}\n", extra.display())).unwrap();
    let (relay, closed) = silent_relay();
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
    // The line the host prints once its frame is done (before it exits and
    // its connections close): how many relay connections it had given up
    // by then.
    let (tx, rx) = mpsc::channel();
    let stdout = child.0.stdout.take().unwrap();
    let c = closed.clone();
    std::thread::spawn(move || {
        let mut out = String::new();
        for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.contains("frames") {
                let _ = tx.send(c.load(Ordering::SeqCst));
            }
            out.push_str(&line);
            out.push('\n');
        }
        out
    });
    let mut opened: Vec<&PathBuf> = Vec::new();
    let status = loop {
        opened.extend(fifos.iter().filter(|f| fifo_has_reader(f)));
        if let Some(s) = child.0.try_wait().unwrap() {
            break s;
        }
        // Only a hang is bounded: a FIFO opened is let go above, and a
        // wait on the relay ends at the host's own timeout.
        assert!(started.elapsed() < Duration::from_secs(120), "no first frame after 120 s");
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut err = String::new();
    child.0.stderr.as_mut().unwrap().read_to_string(&mut err).unwrap();
    assert!(opened.is_empty(), "the host opened a font it was not given: {opened:?}\n{err}");
    assert!(status.success(), "{status}: {err}");
    let given_up = rx.recv().expect("the host's line after its frames");
    assert_eq!(given_up, 0, "the host gave up on the relay before its first frame was done: it waited on it\n{err}");

    // Written: the store (the built-in programs and the manual) and the
    // data directory; nothing else.
    let after: Vec<String> =
        tree(tmp.path()).into_iter().filter(|p| !p.starts_with("store/") && !p.starts_with("data/") && !before.contains(p)).collect();
    assert_eq!(after, Vec::<String>::new());
    assert!(!tree(&at("store")).is_empty(), "the built-in programs are put in the store");
}
