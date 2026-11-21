//! The device bus keeps the machine's semantics (DEVICES.md): the
//! checked-in conformance images and the built-in programs, run with their
//! devices as separate driver processes over both transports, with and
//! without batching, with the C terminal driver and the Python vector
//! driver substituted, give the expected state hashes (or those of the
//! in-process reference, `terse run`'s way of running); a bus recording
//! replays; sections and snapshots round-trip, also between `terse run`
//! and `terse-bus`; a driver that crashes, hangs or is slow to start is
//! handled as DEVICES.md 10 says; the composed picture matches the
//! reference software renderer's. (terse-cli's scripted sessions run on
//! the bus in `sessions.rs`.)

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use terse_bus::bus::BusConfig;
use terse_bus::compose::{Compositor, write_png};
use terse_bus::host::{self, BusSystem, Start};
use terse_bus::inproc::ContentOptions;
use terse_core::{Image, Script, System};
use terse_net::{BlobStore, Draft, HeadlessSystem, Identity, Node, Subscriptions};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn programs() -> PathBuf {
    workspace().join("programs")
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The directory of the driver programs (and of terse and terse-bus).
fn bin_dir() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_terse-dev-terminal")).parent().unwrap().to_path_buf()
}

/// A content store, config and data directory.
struct Dirs {
    _tmp: tempfile::TempDir,
    store: PathBuf,
    config: PathBuf,
    data: PathBuf,
}

impl Dirs {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let (store, config, data) = (tmp.path().join("store"), tmp.path().join("config"), tmp.path().join("data"));
        for d in [&store, &config, &data] {
            std::fs::create_dir_all(d).unwrap();
        }
        Dirs { _tmp: tmp, store, config, data }
    }

    fn options(&self) -> ContentOptions {
        ContentOptions { store: Some(self.store.clone()), config: Some(self.config.clone()), data: Some(self.data.clone()), ..Default::default() }
    }

    fn args(&self) -> Vec<String> {
        let mut v = Vec::new();
        for (k, p) in [("--store", &self.store), ("--config", &self.config), ("--data", &self.data)] {
            v.extend([k.to_string(), p.display().to_string()]);
        }
        v
    }
}

#[derive(Clone, Default)]
struct Run {
    /// Slots → command line replacing the default driver.
    drivers: Vec<(Vec<u8>, Vec<String>)>,
    socket: bool,
    no_batch: bool,
}

/// A started bus system for `image` (a single machine).
fn start(image: &Image, run: &Run, content: &ContentOptions, presents: bool, replay: bool) -> BusSystem {
    let slots = host::needed_slots(image, false);
    let drivers = host::drivers(&bin_dir(), &slots, &content.args(), &run.drivers).unwrap();
    let mut config = BusConfig::new(PathBuf::new());
    config.shm = !run.socket;
    config.presents = presents;
    config.replay = replay;
    config.batching = !run.no_batch;
    Start { config, drivers, image, shell: None, identity: None, snapshot: None, grants: None, without: Vec::new() }.start().unwrap()
}

/// Runs `frames` frames with the script; the hash after every frame that
/// leaves the counter at a multiple of `every`, as `--hash-every` prints.
fn bus_hashes(image: &Image, script: &Script, frames: u32, every: u32, run: &Run, content: &ContentOptions) -> Vec<String> {
    let mut bs = start(image, run, content, false, false);
    let mut out = Vec::new();
    for _ in 0..frames {
        let f = bs.system.shell().frame();
        bs.frame(script.events(f), script.controllers(f)).unwrap();
        let m = bs.system.shell_mut();
        if m.frame() % every == 0 {
            out.push(format!("{} {}", m.frame(), hex(&m.try_state_hash().unwrap())));
        }
        if bs.system.stopped().is_some() {
            break;
        }
    }
    out
}

/// The same with the devices built in, as `terse run` runs a machine.
fn reference_hashes(image: &Image, script: &Script, frames: u32, every: u32, content: &ContentOptions) -> Vec<String> {
    let service = image.lists(4).then(|| content.start().unwrap());
    let mut system = System::new(image, false, Vec::new());
    let mut headless = HeadlessSystem::new(service, script.clone());
    let mut out = Vec::new();
    for _ in 0..frames {
        headless.frame(&mut system);
        let m = system.shell();
        if m.frame() % every == 0 {
            out.push(format!("{} {}", m.frame(), hex(&m.state_hash())));
        }
        if system.stopped().is_some() {
            break;
        }
    }
    out
}

fn script(path: &Path) -> Script {
    Script::parse(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn image(path: &Path) -> Image {
    Image::parse(&std::fs::read(path).unwrap()).unwrap()
}

/// The C terminal driver, built into `dir`, if clang is there.
fn c_terminal(dir: &Path) -> Option<String> {
    let out = dir.join("terse-dev-terminal-c");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("c/terminal.c");
    let ok = Command::new("clang").args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-o"]).arg(&out).arg(&src).status().is_ok_and(|s| s.success());
    if !ok {
        eprintln!("skipped: the C terminal driver needs clang");
        return None;
    }
    Some(out.display().to_string())
}

fn python_vector() -> Option<Vec<String>> {
    if !Command::new("python3").arg("--version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("skipped: the Python vector driver needs python3");
        return None;
    }
    let py = Path::new(env!("CARGO_MANIFEST_DIR")).join("python/vector.py");
    Some(vec!["python3".into(), "-I".into(), py.display().to_string()])
}

#[test]
fn hello_matches_the_state_vectors() {
    // STATE.md 3: hello after 30 frames with no input.
    let img = image(&programs().join("hello/hello.terse"));
    let want = "30 819432152b38626f8fc54b43f50d5d4c501dc99e6904315b61dc78b796c611e2";
    for socket in [false, true] {
        let got = bus_hashes(&img, &Script::default(), 30, 30, &Run { socket, ..Default::default() }, &ContentOptions::default());
        assert_eq!(got, [want], "socket {socket}");
    }
}

/// The runs every conformance check makes: the Rust drivers on both
/// transports, without batching, and the C terminal driver on both.
fn conformance_runs(tmp: &Path) -> Vec<Run> {
    let mut runs = vec![Run::default(), Run { socket: true, ..Default::default() }, Run { no_batch: true, ..Default::default() }];
    if let Some(c) = c_terminal(tmp) {
        runs.push(Run { drivers: vec![(vec![2], vec![c.clone()])], socket: true, ..Default::default() });
        runs.push(Run { drivers: vec![(vec![2], vec!["env".into(), "TERSE_C_SHM=1".into(), c])], ..Default::default() });
    }
    runs
}

/// programs/conform, the full conformance image (devices 2, 3, 4, 7 and 9),
/// with its script: the checked-in hashes every 10 frames to 200.
#[test]
fn conform_hashes() {
    let dir = programs().join("conform");
    let img = image(&dir.join("conform.terse"));
    let s = script(&dir.join("conform.events"));
    let want: Vec<String> = std::fs::read_to_string(dir.join("expected-hashes.txt")).unwrap().lines().map(str::to_string).collect();
    let tmp = tempfile::tempdir().unwrap();
    for run in conformance_runs(tmp.path()) {
        let dirs = Dirs::new();
        let got = bus_hashes(&img, &s, 200, 10, &run, &dirs.options());
        assert_eq!(got, want, "drivers {:?}, socket {}, no batch {}", run.drivers, run.socket, run.no_batch);
    }
}

/// programs/conform-reading with its script: the checked-in hashes.
#[test]
fn conform_reading_hashes() {
    let dir = programs().join("conform-reading");
    let img = image(&dir.join("conform-reading.terse"));
    let s = script(&dir.join("conform-reading.events"));
    let want: Vec<String> = std::fs::read_to_string(dir.join("expected-hashes.txt")).unwrap().lines().map(str::to_string).collect();
    let tmp = tempfile::tempdir().unwrap();
    for run in conformance_runs(tmp.path()) {
        let dirs = Dirs::new();
        let store = BlobStore::open(&dirs.store, None).unwrap();
        for blob in std::fs::read_dir(dir.join("blobs")).unwrap() {
            store.put(&std::fs::read(blob.unwrap().path()).unwrap()).unwrap();
        }
        let got = bus_hashes(&img, &s, 200, 10, &run, &dirs.options());
        assert_eq!(got, want, "drivers {:?}, socket {}, no batch {}", run.drivers, run.socket, run.no_batch);
    }
}

/// programs/conform-program with its script: the checked-in hashes.
#[test]
fn conform_program_hashes() {
    let dir = programs().join("conform-program");
    let img = image(&dir.join("conform-program.terse"));
    let s = script(&dir.join("conform-program.events"));
    let want: Vec<String> = std::fs::read_to_string(dir.join("expected-hashes.txt")).unwrap().lines().map(str::to_string).collect();
    let tmp = tempfile::tempdir().unwrap();
    for run in conformance_runs(tmp.path()) {
        let dirs = Dirs::new();
        let got = bus_hashes(&img, &s, 200, 10, &run, &dirs.options());
        assert_eq!(got, want, "drivers {:?}, socket {}, no batch {}", run.drivers, run.socket, run.no_batch);
    }
}

/// `reference_hashes` with the optional devices `grants` grants.
fn reference_hashes_granted(image: &Image, grants: &terse_core::Grants, script: &Script, frames: u32, every: u32, content: &ContentOptions) -> Vec<String> {
    let service = image.device_set(grants).has(4).then(|| content.start().unwrap());
    let mut system = System::with_machine(terse_core::Machine::with_grants(image, grants.clone()));
    let mut headless = HeadlessSystem::new(service, script.clone());
    let mut out = Vec::new();
    for _ in 0..frames {
        headless.frame(&mut system);
        let m = system.shell();
        if m.frame() % every == 0 {
            out.push(format!("{} {}", m.frame(), hex(&m.state_hash())));
        }
    }
    out
}

/// The hashes of a bus run started with `drivers` (from `dir`), `without`.
fn bus_hashes_with(image: &Image, dir: &Path, without: &[u8], script: &Script, frames: u32, every: u32, content: &ContentOptions) -> Result<Vec<String>, String> {
    let slots = host::installed_slots(dir, image, false, without, &[]);
    let drivers = host::drivers(dir, &slots, &content.args(), &[])?;
    let start = Start { config: BusConfig::new(PathBuf::new()), drivers, image, shell: None, identity: None, snapshot: None, grants: None, without: without.to_vec() };
    let mut bs = start.start()?;
    let mut out = Vec::new();
    for _ in 0..frames {
        let f = bs.system.shell().frame();
        bs.frame(script.events(f), script.controllers(f))?;
        let m = bs.system.shell_mut();
        if m.frame() % every == 0 {
            out.push(format!("{} {}", m.frame(), hex(&m.try_state_hash()?)));
        }
    }
    Ok(out)
}

/// SPEC §image: sparks lists content (device 4) and full text (10) as
/// optional. On the bus with content, with content withheld (`without`),
/// and with no content driver installed, it gives the hashes of the
/// devices compiled in with the same grants; without either (the program
/// profile) they are the golden `program-hashes.txt`. An image that
/// requires content (fetch) still cannot start without its driver.
#[test]
fn optional_devices_on_the_bus() {
    let img = image(&programs().join("sparks/sparks.terse"));
    let s = script(&programs().join("sparks/sessions/keys.txt"));
    let with = terse_core::Grants(vec![true, true]);
    let without = terse_core::Grants(vec![false, true]);
    let program = terse_core::Grants(vec![false, false]);
    let dirs = Dirs::new();
    let want_with = reference_hashes_granted(&img, &with, &s, 900, 30, &dirs.options());
    let dirs = Dirs::new();
    let want_without = reference_hashes_granted(&img, &without, &s, 900, 30, &dirs.options());
    assert_ne!(want_with, want_without);
    let dirs = Dirs::new();
    let want_program = reference_hashes_granted(&img, &program, &s, 900, 30, &dirs.options());
    let golden: Vec<String> = std::fs::read_to_string(programs().join("sparks/expected/program-hashes.txt")).unwrap().lines().take(2).map(str::to_string).collect();
    assert_eq!(golden, [want_program[0].clone(), want_program[29].clone()]);
    let dirs = Dirs::new();
    assert_eq!(bus_hashes_with(&img, &bin_dir(), &[4, 10], &s, 900, 30, &dirs.options()).unwrap(), want_program, "the program profile");

    let dirs = Dirs::new();
    assert_eq!(bus_hashes_with(&img, &bin_dir(), &[], &s, 900, 30, &dirs.options()).unwrap(), want_with, "content granted");
    let dirs = Dirs::new();
    assert_eq!(bus_hashes_with(&img, &bin_dir(), &[4], &s, 900, 30, &dirs.options()).unwrap(), want_without, "content withheld");
    // A directory with the terminal and vector drivers only.
    let partial = tempfile::tempdir().unwrap();
    for name in ["terse-dev-terminal", "terse-dev-vector"] {
        std::os::unix::fs::symlink(bin_dir().join(name), partial.path().join(name)).unwrap();
    }
    assert_eq!(host::installed_slots(partial.path(), &img, false, &[], &[]), [2, 3, 9, 10]);
    let dirs = Dirs::new();
    assert_eq!(bus_hashes_with(&img, partial.path(), &[], &s, 900, 30, &dirs.options()).unwrap(), want_without, "no content driver");
    let fetch = image(&programs().join("fetch/fetch.terse"));
    assert!(host::installed_slots(partial.path(), &fetch, false, &[], &[]).contains(&4));
    let dirs = Dirs::new();
    assert!(bus_hashes_with(&fetch, partial.path(), &[], &s, 1, 1, &dirs.options()).is_err(), "a required device without its driver");
}

/// Snapshots carry the grants between hosts: one terse-bus takes without
/// content restores in `terse run` on a full host and goes on without it;
/// one taken with content does not restore in terse-bus without content.
#[test]
fn optional_snapshots_cross_hosts() {
    if !terse().exists() || !terse_bus().exists() {
        eprintln!("skipped: terse and terse-bus are not built");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let p = |n: &str| tmp.path().join(n).display().to_string();
    let dirs = Dirs::new();
    let img = programs().join("sparks/sparks.terse").display().to_string();
    let events = programs().join("sparks/sessions/keys.txt").display().to_string();
    let run = |prog: &Path, extra: &[&str]| {
        let mut c = Command::new(prog);
        if prog == terse().as_path() {
            c.arg("run");
        }
        c.args([&img, "--events", &events]).args(dirs.args()).args(extra);
        c.output().unwrap()
    };
    let ok = |o: std::process::Output| {
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    };
    let whole = ok(run(&terse(), &["--without", "4", "--frames", "600", "--hash-every", "300"]));
    ok(run(&terse_bus(), &["--without", "4", "--frames", "300", "--save-state", &p("without.snap")]));
    let later = ok(run(&terse(), &["--frames", "300", "--hash-every", "300", "--load-state", &p("without.snap")]));
    assert_eq!(later, whole.lines().nth(1).unwrap().to_string() + "\n");
    ok(run(&terse(), &["--frames", "300", "--save-state", &p("with.snap")]));
    let refused = run(&terse_bus(), &["--without", "4", "--frames", "1", "--load-state", &p("with.snap")]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("device 4"), "{}", String::from_utf8_lossy(&refused.stderr));
}

/// programs/sparks with its recorded keyboard session: `keys-hashes.txt`
/// at frames 30 and 900, with the Rust drivers, and with the C terminal
/// and the Python vector display.
#[test]
fn sparks_hashes() {
    let img = image(&programs().join("sparks/sparks.terse"));
    let s = script(&programs().join("sparks/sessions/keys.txt"));
    let want: Vec<String> = std::fs::read_to_string(programs().join("sparks/expected/keys-hashes.txt")).unwrap().lines().take(2).map(str::to_string).collect();
    let tmp = tempfile::tempdir().unwrap();
    let mut runs = vec![Run::default()];
    let mut mixed = Run::default();
    if let Some(c) = c_terminal(tmp.path()) {
        mixed.drivers.push((vec![2], vec![c]));
    }
    if let Some(p) = python_vector() {
        mixed.drivers.push((vec![3], p));
    }
    if !mixed.drivers.is_empty() {
        runs.push(mixed);
    }
    for run in runs {
        let dirs = Dirs::new();
        let got = bus_hashes(&img, &s, 900, 1, &run, &dirs.options());
        let picked: Vec<String> = got.into_iter().filter(|l| l.starts_with("30 ") || l.starts_with("900 ")).collect();
        assert_eq!(picked, want, "drivers {:?}", run.drivers);
    }
}

/// A store with the four demo articles (pictures and a table among them)
/// in a feed the config follows.
fn reader_fixture() -> Dirs {
    let dirs = Dirs::new();
    let mut node = Node::new(BlobStore::open(&dirs.store, None).unwrap(), None);
    let alice = Identity::from_seed([11; 32]);
    node.set_identity(alice.clone());
    let demo = workspace().join("crates/terse-host/demo");
    for (i, (title, file)) in [("How Tersenet works", "long.txt"), ("Hello in three languages", "languages.txt"), ("Welcome to Tersenet", "welcome.txt"), ("Pictures in articles", "pictures.txt")].iter().enumerate() {
        let draft = Draft { data: std::fs::read(demo.join(file)).unwrap(), title: title.to_string(), reply_to: None, time: 1_794_938_400 + 600 * i as u32 };
        assert!(matches!(node.publish_article(&draft), terse_net::Done::Published { .. }));
    }
    let mut subs = Subscriptions::load(dirs.config.join("subscriptions")).unwrap();
    subs.add(alice.feed_key()).unwrap();
    dirs
}

const READER_SCRIPT: &str = "40 press enter\n80 press down\n82 press down\n84 wheel 3\n90 press pagedown\n130 press escape\n\
150 press down\n160 press enter\n200 press pagedown\n230 press escape\n250 text /\n255 text terse\n260 press enter\n";

/// The reader: the list, an article with figures, scrolling, an article
/// with a table, search: the same hashes as with the devices built in.
#[test]
fn reader_hashes() {
    let img = image(&programs().join("reader/reader.terse"));
    let s = Script::parse(READER_SCRIPT).unwrap();
    let dirs = reader_fixture();
    let want = reference_hashes(&img, &s, 320, 10, &dirs.options());
    assert_eq!(want.len(), 32);
    for run in [Run::default(), Run { socket: true, no_batch: true, ..Default::default() }] {
        assert_eq!(bus_hashes(&img, &s, 320, 10, &run, &dirs.options()), want, "socket {}", run.socket);
    }
}

/// A bus recording replays with the same hashes, without the store.
#[test]
fn record_and_replay() {
    let img = image(&programs().join("reader/reader.terse"));
    let s = Script::parse(READER_SCRIPT).unwrap();
    let dirs = reader_fixture();
    let mut bs = start(&img, &Run::default(), &dirs.options(), false, false);
    let mut frames = Vec::new();
    for _ in 0..200 {
        let f = bs.system.shell().frame();
        bs.frame(s.events(f), s.controllers(f)).unwrap();
        let m = bs.system.shell_mut();
        let records = m.devices_mut().take_records();
        frames.push((f, s.events(f).to_vec(), records, m.try_state_hash().unwrap()));
    }
    drop(bs);
    assert!(frames.iter().any(|f| !f.2.is_empty()), "the content and page drivers recorded inputs");
    // Replay with no store at all: the content driver must not need one.
    let none = ContentOptions { store: Some("/nonexistent/terse-bus-test".into()), ..Default::default() };
    let mut bs = start(&img, &Run { socket: true, ..Default::default() }, &none, false, true);
    for (f, events, records, hash) in &frames {
        assert_eq!(bs.system.shell().frame(), *f);
        bs.system.shell_mut().devices_mut().replay = Some(records.clone());
        bs.frame(events, Default::default()).unwrap();
        assert_eq!(bs.system.shell_mut().try_state_hash().unwrap(), *hash, "frame {f}");
    }
}

/// STATE then RESTORE round-trips every driver's section, a cut one is
/// refused, and a whole serialization loads into a fresh machine.
#[test]
fn state_restore() {
    let img = image(&programs().join("conform/conform.terse"));
    let s = script(&programs().join("conform/conform.events"));
    let dirs = Dirs::new();
    let mut bs = start(&img, &Run::default(), &dirs.options(), false, false);
    for _ in 0..60 {
        let f = bs.system.shell().frame();
        bs.frame(s.events(f), s.controllers(f)).unwrap();
    }
    let m = bs.system.shell_mut();
    for slot in [2u8, 3, 4, 7, 9] {
        let d = m.devices_mut();
        let section = d.state(slot).unwrap();
        let mut rest = section.clone();
        rest.extend_from_slice(b"more");
        assert_eq!(d.restore(slot, &rest).unwrap(), Some(section.len()), "slot {slot}");
        assert_eq!(d.state(slot).unwrap(), section);
        if slot != 9 {
            assert_eq!(d.restore(slot, &section[..section.len() - 1]).unwrap(), None, "slot {slot} took a cut section");
        }
    }
    let state = m.try_state().unwrap();
    let snapshot = terse_core::Snapshot::try_of(m).unwrap();
    drop(bs);
    let config = BusConfig::new(PathBuf::new());
    let drivers = host::drivers(&bin_dir(), &host::needed_slots(&img, false), &dirs.options().args(), &[]).unwrap();
    let mut again = Start { config, drivers, image: &img, shell: None, identity: None, snapshot: Some(&snapshot), grants: None, without: Vec::new() }.start().unwrap();
    assert_eq!(again.system.shell_mut().try_state().unwrap(), state);
}

fn terse() -> PathBuf {
    bin_dir().join("terse")
}

fn terse_bus() -> PathBuf {
    bin_dir().join("terse-bus")
}

fn stdout(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    assert!(out.status.success() || out.status.code() == Some(0), "{cmd:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

/// Snapshot files are the same in both hosts (STATE.md 4): one `terse run
/// --save-state` writes restores in `terse-bus --load-state` and goes on
/// with the hashes of the whole run, and the other way round; both write
/// the same file for the same run.
#[test]
fn snapshots_cross_hosts() {
    if !terse().exists() || !terse_bus().exists() {
        eprintln!("skipped: terse and terse-bus are not built");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let p = |n: &str| tmp.path().join(n).display().to_string();
    let dirs = Dirs::new();
    let img = programs().join("conform/conform.terse").display().to_string();
    let events = programs().join("conform/conform.events").display().to_string();
    let run = |prog: &Path, extra: &[&str]| {
        let mut c = Command::new(prog);
        if prog == terse().as_path() {
            c.arg("run");
        }
        c.args([&img, "--events", &events]).args(dirs.args()).args(extra);
        stdout(&mut c)
    };
    let a = run(&terse(), &["--frames", "100", "--hash", "--save-state", &p("a.snap")]);
    let b = run(&terse_bus(), &["--frames", "100", "--hash", "--save-state", &p("b.snap")]);
    assert_eq!(a, b, "the same run, the same hash");
    assert_eq!(std::fs::read(p("a.snap")).unwrap(), std::fs::read(p("b.snap")).unwrap(), "the same snapshot file");
    let whole = run(&terse(), &["--frames", "200", "--hash-every", "50"]);
    let later: String = whole.lines().filter(|l| l.starts_with("150 ") || l.starts_with("200 ")).map(|l| format!("{l}\n")).collect();
    // Each host goes on from the other's snapshot as the whole run did
    // (the script's later frames: --events is counted from frame 0).
    let from_terse = run(&terse_bus(), &["--frames", "100", "--hash-every", "50", "--load-state", &p("a.snap")]);
    let from_bus = run(&terse(), &["--frames", "100", "--hash-every", "50", "--load-state", &p("b.snap")]);
    assert_eq!(from_terse, later, "terse-bus restored terse run's snapshot");
    assert_eq!(from_bus, later, "terse run restored terse-bus's snapshot");
}

/// The processes whose command line names `needle`.
fn processes_with(needle: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        if let Ok(cmd) = std::fs::read(e.path().join("cmdline"))
            && String::from_utf8_lossy(&cmd).contains(needle)
        {
            out.push(pid);
        }
    }
    out
}

/// A copy of the crash test driver under a name of its own, so that its
/// processes can be found.
fn crash_driver(dir: &Path, name: &str) -> String {
    let path = dir.join(name);
    std::fs::copy(env!("CARGO_BIN_EXE_terse-dev-crash"), &path).unwrap();
    path.display().to_string()
}

/// DEVICES.md 10: a driver that exits in the middle of a frame, one that
/// hangs, and one that is slow to start. The run stops with status 3 and
/// the driver named; nothing is left running and the session's files are
/// gone; a slow driver is waited for, up to --start-timeout.
#[test]
fn driver_failures() {
    if !terse_bus().exists() {
        eprintln!("skipped: terse-bus is not built");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let hello = programs().join("hello/hello.terse").display().to_string();
    let cases = [("crash-exit", "TERSE_CRASH=exit TERSE_CRASH_AT=40", "--driver-timeout", "5"), ("crash-hang", "TERSE_CRASH=hang TERSE_CRASH_AT=40", "--driver-timeout", "2")];
    for (name, env, opt, v) in cases {
        let exe = crash_driver(tmp.path(), name);
        let cmd = format!("env {env} {exe}");
        let start = Instant::now();
        let out = bus_output([&hello, "--frames", "100", "--hash", "--driver", &format!("2={cmd}"), opt, v]);
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(3), "{name}: {err}");
        assert!(err.contains("driver") && err.contains(name), "{name}: {err}");
        assert!(start.elapsed() < Duration::from_secs(20), "{name}: took {:?}", start.elapsed());
        assert!(processes_with(&exe).is_empty(), "{name}: a driver was left running");
    }
    // Slow to start: two seconds, within the default ten.
    let exe = crash_driver(tmp.path(), "slow-start");
    let cmd = format!("env TERSE_CRASH=slow TERSE_CRASH_THEN={} {exe}", env!("CARGO_BIN_EXE_terse-dev-terminal"));
    let out = bus_output([&hello, "--frames", "30", "--hash", "--driver", &format!("2={cmd}")]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "819432152b38626f8fc54b43f50d5d4c501dc99e6904315b61dc78b796c611e2");
    // ... and too slow for a one-second limit.
    let out = bus_output([&hello, "--frames", "30", "--driver", &format!("2={cmd}"), "--start-timeout", "1"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("did not connect"), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(processes_with(&exe).is_empty(), "slow start: a driver was left running");
}

/// terse-bus's output; it must have left no session files behind.
fn bus_output<const N: usize>(args: [&str; N]) -> std::process::Output {
    let child = Command::new(terse_bus()).args(args).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
    let pid = child.id();
    let out = child.wait_with_output().unwrap();
    let left = std::fs::read_dir("/dev/shm").map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&format!("terse-bus-{pid}-"))).count()).unwrap_or(0);
    assert_eq!(left, 0, "terse-bus {args:?} left its session files");
    out
}

/// SIGTERM and SIGINT end a run cleanly: the drivers are shut down and
/// the session's files removed.
#[test]
fn signals_end_a_run_cleanly() {
    if !terse_bus().exists() {
        eprintln!("skipped: terse-bus is not built");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let sparks = programs().join("sparks/sparks.terse").display().to_string();
    for (sig, name) in [(libc_sigterm(), "term"), (libc_sigint(), "int")] {
        // A copy of the terminal driver, to find its process.
        let exe = tmp.path().join(format!("terse-dev-terminal-{name}"));
        std::fs::copy(env!("CARGO_BIN_EXE_terse-dev-terminal"), &exe).unwrap();
        // Killed when dropped, so that a failing assertion leaves no run
        // of 100,000 frames behind.
        let mut child = KillOnDrop(
            Command::new(terse_bus())
                .args([&sparks, "--realtime", "--frames", "100000", "--driver", &format!("2={}", exe.display())])
                .spawn()
                .unwrap(),
        );
        let child = &mut child.0;
        let deadline = Instant::now() + Duration::from_secs(20);
        while processes_with(&exe.display().to_string()).is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(500));
        let session: Vec<_> = std::fs::read_dir("/dev/shm").unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains(&format!("terse-bus-{}-", child.id()))).collect();
        assert_eq!(session.len(), 1, "the run's session directory");
        // SAFETY: a signal to our own child.
        unsafe {
            kill(child.id() as i32, sig);
        }
        let status = child.wait().unwrap();
        assert!(status.success(), "{name}: {status}");
        assert!(processes_with(&exe.display().to_string()).is_empty(), "{name}: the driver was left running");
        assert!(!session[0].path().exists(), "{name}: the session directory was left");
    }
}

unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

/// A child process killed (and reaped) when dropped, unless it has ended.
struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Ok(None) = self.0.try_wait() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn libc_sigterm() -> i32 {
    15
}

fn libc_sigint() -> i32 {
    2
}

// Pictures.

fn png_rgb(path: &Path) -> (u32, u32, Vec<u8>) {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    let ch = info.color_type.samples();
    let rgb = buf[..info.buffer_size()].chunks(ch).flat_map(|p| [p[0], p[1], p[2]]).collect();
    (info.width, info.height, rgb)
}

/// The reference software renderer's tolerance (terse-host's reference
/// pictures): at most 0.5 % of the pixels off by more than 8, mean under 0.5.
fn assert_close(a: &Path, b: &Path) {
    let (aw, ah, pa) = png_rgb(a);
    let (bw, bh, pb) = png_rgb(b);
    assert_eq!((aw, ah), (bw, bh));
    let mut off = 0;
    let mut sum = 0u64;
    for (x, y) in pa.chunks(3).zip(pb.chunks(3)) {
        let d = (0..3).map(|i| (x[i] as i32 - y[i] as i32).unsigned_abs()).max().unwrap();
        off += (d > 8) as usize;
        sum += (0..3).map(|i| (x[i] as i32 - y[i] as i32).unsigned_abs() as u64).sum::<u64>();
    }
    let n = (aw * ah) as f64;
    let (off, mean) = (off as f64 / n, sum as f64 / (n * 3.0));
    assert!(off <= 0.005 && mean < 0.5, "{} vs {}: {:.3} % off, mean {mean:.3}", a.display(), b.display(), off * 100.0);
}

/// Whether the fonts the reference pictures assume are here.
fn have_fonts() -> bool {
    let ok = terse_host::page::Typesetter::new(&Default::default()).is_ok();
    if !ok {
        eprintln!("skipped: no fonts for pictures");
    }
    ok
}

/// The bus's composed picture after `frames` frames.
fn bus_picture(img: &Image, s: &Script, frames: u32, size: (u32, u32), content: &ContentOptions, out: &Path) {
    let mut bs = start(img, &Run::default(), content, true, false);
    for _ in 0..frames {
        let f = bs.system.shell().frame();
        bs.frame(s.events(f), s.controllers(f)).unwrap();
    }
    let native = bs.bus.borrow().native().unwrap();
    let mut comp = Compositor::new(bs.system.shell().devices(), native, 0);
    let frame = comp.draw(&mut bs.system, size, None).unwrap();
    write_png(out, &frame.to_rgba(), size).unwrap();
}

/// terse-host's software renderer's picture.
fn host_picture(image: &Path, events: Option<&Path>, frames: u32, size: (u32, u32), dirs: &Dirs, out: &Path) {
    let mut args = vec![image.display().to_string(), "--screenshot".into(), out.display().to_string(), "--frames".into(), frames.to_string()];
    args.extend(["--size".into(), format!("{}x{}", size.0, size.1), "--renderer".into(), "software".into()]);
    args.extend(dirs.args());
    if let Some(e) = events {
        args.extend(["--events".into(), e.display().to_string()]);
    }
    let _ = terse_host::run(args).unwrap();
}

#[test]
fn pictures_match_the_software_renderer() {
    if !have_fonts() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let size = (800, 600);
    // hello; sparks (the clean style) after 300 frames of play.
    for (name, events, frames) in [("hello", None, 30), ("sparks", Some(programs().join("sparks/sessions/keys.txt")), 300)] {
        let path = programs().join(format!("{name}/{name}.terse"));
        let img = image(&path);
        let s = events.as_ref().map_or_else(Script::default, |e| script(e));
        let (a, b) = (tmp.path().join(format!("{name}-host.png")), tmp.path().join(format!("{name}-bus.png")));
        let (d1, d2) = (Dirs::new(), Dirs::new());
        host_picture(&path, events.as_deref(), frames, size, &d1, &a);
        bus_picture(&img, &s, frames, size, &d2.options(), &b);
        assert_close(&a, &b);
    }
    // The reader: an article with figures, one with a table, search.
    let path = programs().join("reader/reader.terse");
    let img = image(&path);
    let events = tmp.path().join("reader.events");
    std::fs::write(&events, READER_SCRIPT).unwrap();
    let s = Script::parse(READER_SCRIPT).unwrap();
    let dirs = reader_fixture();
    for frames in [120, 220, 300] {
        let (a, b) = (tmp.path().join(format!("reader-{frames}-host.png")), tmp.path().join(format!("reader-{frames}-bus.png")));
        host_picture(&path, Some(&events), frames, size, &dirs, &a);
        bus_picture(&img, &s, frames, size, &dirs.options(), &b);
        assert_close(&a, &b);
    }
}
