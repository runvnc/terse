//! The device bus keeps the machine's semantics (DEVICES.md): the
//! checked-in conformance image and the built-in programs, run with their
//! devices as separate driver processes over both transports, with and
//! without batching, with the C terminal driver and the Python vector
//! driver substituted, give the expected state hashes (or those of the
//! in-process reference, `terse run`'s way of running); a bus recording
//! replays; the composed layers match the reference software renderer's
//! picture.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use terse_bus::bus::{Bus, BusConfig, SharedMemory};
use terse_bus::compose::{Compositor, screen_rect};
use terse_bus::inproc::ContentOptions;
use terse_bus::proto::MEMORY_RAM_OFFSET;
use terse_bus::transport::{self, Mapping};
use terse_core::devices::Memory;
use terse_core::{FrameInput, Image, Machine, Script, System};
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
}

#[derive(Clone, Default)]
struct Run {
    /// slot → command line replacing the default driver.
    drivers: Vec<(u8, Vec<String>)>,
    socket: bool,
    no_batch: bool,
}

fn default_driver(slot: u8, content: &ContentOptions) -> Vec<String> {
    match slot {
        2 => vec![env!("CARGO_BIN_EXE_terse-dev-terminal").to_string()],
        3 => vec![env!("CARGO_BIN_EXE_terse-dev-vector").to_string()],
        4 => {
            let mut v = vec![env!("CARGO_BIN_EXE_terse-dev-content").to_string()];
            v.extend(content.args());
            v
        }
        7 => vec![env!("CARGO_BIN_EXE_terse-dev-page").to_string()],
        s => panic!("no driver for slot {s}"),
    }
}

/// A started bus core for `image`.
fn start(image: &Image, run: &Run, content: &ContentOptions, presents: bool, replay: bool) -> (Machine<Bus>, PathBuf) {
    let session = transport::session_dir().unwrap();
    let ram_len = image.ram_pages as usize * 8192;
    let memory_path = session.join("memory");
    let mut map = Mapping::create(&memory_path, MEMORY_RAM_OFFSET + ram_len).unwrap();
    map.bytes_mut()[..image.rom.len()].copy_from_slice(&image.rom);
    let mut wanted = Vec::new();
    for d in image.devices.iter().filter(|d| d.slot != 1) {
        let cmd = run.drivers.iter().find(|(s, _)| *s == d.slot).map(|(_, c)| c.clone()).unwrap_or_else(|| default_driver(d.slot, content));
        wanted.push((d.slot, d.version, cmd));
    }
    wanted.sort_by_key(|w| w.0);
    let config = BusConfig {
        session: session.clone(),
        shm: !run.socket,
        presents,
        replay,
        batching: !run.no_batch,
        spin: Duration::from_micros(100),
        driver_cpus: Vec::new(),
    };
    let bus = Bus::start(&config, &wanted, image.id, &image.rom, ram_len as u32, Some(&memory_path)).unwrap();
    let memory = Memory::Backed(Box::new(SharedMemory { map, rom_len: image.rom.len(), ram_len }));
    (Machine::with_devices(image, memory, bus), session)
}

/// Runs `frames` frames with the script; the hash after every frame that
/// leaves the counter at a multiple of `every`, as `--hash-every` prints.
fn bus_hashes(image: &Image, script: &Script, frames: u32, every: u32, run: &Run, content: &ContentOptions) -> Vec<String> {
    let (mut core, session) = start(image, run, content, false, false);
    let mut out = Vec::new();
    for _ in 0..frames {
        let f = core.frame();
        core.try_run_frame(&input(script, f)).unwrap();
        if core.frame() % every == 0 {
            out.push(format!("{} {}", core.frame(), hex(&core.try_state_hash().unwrap())));
        }
        if core.stopped().is_some() {
            break;
        }
    }
    core.devices_mut().shutdown();
    let _ = std::fs::remove_dir_all(session);
    out
}

/// The same with the devices built in, as `terse run` runs a machine.
fn reference_hashes(image: &Image, script: &Script, frames: u32, every: u32, content: &ContentOptions) -> Vec<String> {
    let service = image.has_device(4).then(|| content.start().unwrap());
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

/// Frame `f`'s input from the script.
fn input(s: &Script, f: u32) -> FrameInput<'_> {
    FrameInput { events: s.events(f), controllers: s.controllers(f), ..Default::default() }
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
    let want = "30 066e293e97fd6d5908db87cc110ebc46846d73b310a7047e1f888c7b010e2c83";
    for socket in [false, true] {
        let got = bus_hashes(&img, &Script::default(), 30, 30, &Run { socket, ..Default::default() }, &ContentOptions::default());
        assert_eq!(got, [want], "socket {socket}");
    }
}

/// programs/conform-reading with its script: the checked-in hashes, every
/// 10 frames to 200, with the Rust drivers on both transports, without
/// batching, and with the C terminal driver on both transports.
#[test]
fn conform_reading_hashes() {
    let dir = programs().join("conform-reading");
    let img = image(&dir.join("conform-reading.terse"));
    let s = script(&dir.join("conform-reading.events"));
    let want: Vec<String> = std::fs::read_to_string(dir.join("expected-hashes.txt")).unwrap().lines().map(str::to_string).collect();
    let tmp = tempfile::tempdir().unwrap();
    let mut runs = vec![Run::default(), Run { socket: true, ..Default::default() }, Run { no_batch: true, ..Default::default() }];
    if let Some(c) = c_terminal(tmp.path()) {
        runs.push(Run { drivers: vec![(2, vec![c.clone()])], socket: true, ..Default::default() });
        runs.push(Run { drivers: vec![(2, vec!["env".into(), "TERSE_C_SHM=1".into(), c])], ..Default::default() });
    }
    for run in runs {
        let dirs = Dirs::new();
        let store = BlobStore::open(&dirs.store, None).unwrap();
        for blob in std::fs::read_dir(dir.join("blobs")).unwrap() {
            store.put(&std::fs::read(blob.unwrap().path()).unwrap()).unwrap();
        }
        let got = bus_hashes(&img, &s, 200, 10, &run, &dirs.options());
        assert_eq!(got, want, "drivers {:?}, socket {}, no batch {}", run.drivers, run.socket, run.no_batch);
    }
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
        mixed.drivers.push((2, vec![c]));
    }
    if let Some(p) = python_vector() {
        mixed.drivers.push((3, p));
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
    for (i, (title, file)) in [("How Terse works", "long.txt"), ("Hello in three languages", "languages.txt"), ("Welcome to Terse", "welcome.txt"), ("Pictures in articles", "pictures.txt")].iter().enumerate() {
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
    let (mut core, session) = start(&img, &Run::default(), &dirs.options(), false, false);
    let mut frames = Vec::new();
    for _ in 0..200 {
        let f = core.frame();
        core.try_run_frame(&input(&s, f)).unwrap();
        let records = std::mem::take(&mut core.devices_mut().records);
        frames.push((f, s.events(f).to_vec(), records, core.try_state_hash().unwrap()));
    }
    core.devices_mut().shutdown();
    let _ = std::fs::remove_dir_all(session);
    assert!(frames.iter().any(|f| !f.2.is_empty()), "the content and page drivers recorded inputs");
    // Replay with no store at all: the content driver must not need one.
    let none = ContentOptions { store: Some("/nonexistent/terse-bus-test".into()), ..Default::default() };
    let (mut core, session) = start(&img, &Run { socket: true, ..Default::default() }, &none, false, true);
    for (f, events, records, hash) in &frames {
        assert_eq!(core.frame(), *f);
        core.devices_mut().replay = Some(records.clone());
        core.try_run_frame(&FrameInput { events, ..Default::default() }).unwrap();
        assert_eq!(core.try_state_hash().unwrap(), *hash, "frame {f}");
    }
    core.devices_mut().shutdown();
    let _ = std::fs::remove_dir_all(session);
}

/// STATE then RESTORE round-trips every driver's section, and a malformed
/// one is refused.
#[test]
fn state_restore() {
    let img = image(&programs().join("conform-reading/conform-reading.terse"));
    let s = script(&programs().join("conform-reading/conform-reading.events"));
    let dirs = Dirs::new();
    let (mut core, session) = start(&img, &Run::default(), &dirs.options(), false, false);
    for _ in 0..60 {
        let f = core.frame();
        core.try_run_frame(&input(&s, f)).unwrap();
    }
    for slot in [2u8, 4, 7] {
        let section = core.devices_mut().state(slot).unwrap();
        assert!(core.devices_mut().restore(slot, &section).unwrap(), "slot {slot}");
        assert_eq!(core.devices_mut().state(slot).unwrap(), section);
        assert!(!core.devices_mut().restore(slot, &section[..section.len() - 1]).unwrap(), "slot {slot} took a cut section");
    }
    core.devices_mut().shutdown();
    let _ = std::fs::remove_dir_all(session);
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
    let ok = terse_host::glyphs::SystemGlyphs::new(terse_host::FONT_PX, None).is_ok() && terse_host::page::Typesetter::new(&Default::default()).is_ok();
    if !ok {
        eprintln!("skipped: no fonts for pictures");
    }
    ok
}

/// The bus's composed picture after `frames` frames.
fn bus_picture(img: &Image, s: &Script, frames: u32, size: (u32, u32), content: &ContentOptions, out: &Path) {
    let (mut core, session) = start(img, &Run::default(), content, true, false);
    for _ in 0..frames {
        let f = core.frame();
        core.try_run_frame(&input(s, f)).unwrap();
    }
    let native = core.devices_mut().native().unwrap();
    let order = core.devices_mut().layers().into_iter().map(|(_, s, _, l)| (s, l.id)).collect();
    let mut comp = Compositor::new(size, order);
    let updates = core.devices_mut().present(size, screen_rect(native, size)).unwrap();
    comp.update(&updates).unwrap();
    terse_bus::compose::write_png(out, &comp.rgba(), size).unwrap();
    core.devices_mut().shutdown();
    let _ = std::fs::remove_dir_all(session);
}

/// terse-host's software renderer's picture.
fn host_picture(image: &Path, events: Option<&Path>, frames: u32, size: (u32, u32), dirs: &Dirs, out: &Path) {
    let mut args = vec![image.display().to_string(), "--screenshot".into(), out.display().to_string(), "--frames".into(), frames.to_string()];
    args.extend(["--size".into(), format!("{}x{}", size.0, size.1), "--renderer".into(), "software".into()]);
    args.extend(["--store".into(), dirs.store.display().to_string(), "--config".into(), dirs.config.display().to_string()]);
    args.extend(["--data".to_string(), dirs.data.display().to_string()]);
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
