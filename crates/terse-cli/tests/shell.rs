//! programs/shell/shell.terse: the shell and the machines it starts (SPEC
//! 12), run as `terse run --shell` runs them, with the built-in programs in a
//! temporary store. Golden screens are in programs/shell/expected/ (the
//! shell's screen, then each placed machine's header line and screen, as
//! `terse run --shell --dump` prints them; UPDATE_GOLDEN=1 rewrites them).

mod common;

use common::FrameStats;
use terse_core::machines::{picture, state};
use terse_core::page::reference_layout;
use terse_core::{Event, Image, Machine, Script, System};
use terse_net::{BlobStore, Consent, ContentService, Draft, HeadlessSystem, Identity, Node, Subscriptions, builtin, to_hex};

const SHELL: &[u8] = include_bytes!("../../../programs/shell/shell.terse");

/// A program that writes "crash" and then loads from address 0: a load
/// fault in its first frame.
fn crash_image() -> Vec<u8> {
    let code: [u32; 6] = [
        0x0000_0517, // auipc a0, 0
        0x0185_0513, // addi a0, a0, 24: the text after the code
        0x0050_0593, // li a1, 5
        0x2000_0893, // li a7, 0x200 (write)
        0x0000_0073, // ecall
        0x0000_2503, // lw a0, 0(x0)
    ];
    let mut rom: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    rom.extend_from_slice(b"crash");
    Image::build("crash", 1, &[terse_core::DeviceReq { slot: 2, version: 0 }], &rom)
}

/// A blob ID that is in no store.
const UNKNOWN: [u8; 32] = [7; 32];

/// The author of the one article the reader finds.
fn alice() -> Identity {
    Identity::from_seed([11; 32])
}

struct Sys {
    system: System,
    headless: HeadlessSystem,
    _store: tempfile::TempDir,
    /// Units the shell used in each frame.
    shell_units: Vec<u64>,
}

/// What a run has: extra built-in images after the four, an ID in no
/// store after them, an article for the reader.
#[derive(Default, Clone, Copy)]
struct Setup {
    crash: bool,
    unknown: bool,
    article: bool,
}

impl Sys {
    fn new(script: &str, setup: Setup) -> Self {
        let store = tempfile::tempdir().unwrap();
        let mut node = Node::new(BlobStore::open(store.path(), None).unwrap(), None);
        if setup.article {
            node.set_identity(alice());
            let draft = Draft { data: b"Tiles are machines.\n".to_vec(), title: "On tiles".into(), reply_to: None, time: 1_800_000_000 };
            assert!(matches!(node.publish_article(&draft), terse_net::Done::Published { .. }));
            node.set_subscriptions(Subscriptions::in_memory([alice().feed_key()]));
        }
        let extra: Vec<Vec<u8>> = if setup.crash { vec![crash_image()] } else { Vec::new() };
        let mut ids = builtin::install(node.store(), &extra).unwrap();
        if setup.unknown {
            ids.push(UNKNOWN);
        }
        let content = ContentService::start_with_consent(node, Consent::Deny);
        let system = System::new(&Image::parse(SHELL).unwrap(), true, ids);
        let headless = HeadlessSystem::new(Some(content), Script::parse(script).unwrap());
        Sys { system, headless, _store: store, shell_units: Vec::new() }
    }

    fn run(&mut self, frames: u32) {
        for _ in 0..frames {
            let r = self.headless.frame(&mut self.system);
            self.shell_units.push(r.shell.expect("the shell runs").used);
            assert!(self.system.shell().stopped().is_none(), "{:?}", self.system.shell().stopped());
        }
    }

    fn machine(&self, h: u8) -> &Machine {
        self.system.child(h).and_then(|c| c.machine.as_ref()).unwrap_or_else(|| panic!("machine {h}"))
    }

    fn state(&self, h: u8) -> u8 {
        self.system.state(h)
    }

    fn rect(&self, h: u8) -> (u16, u16, u16, u16) {
        let r = self.system.shell().machines().unwrap().slot(h).rect;
        (r.x, r.y, r.w, r.h)
    }

    /// Row 0 of the shell's screen.
    fn bar(&self) -> String {
        self.system.shell().terminal().unwrap().text().lines().next().unwrap().trim_end().to_string()
    }

    fn screen(m: &Machine) -> String {
        m.terminal().map_or(String::new(), |t| t.text())
    }

    /// The shell's page (its launcher), as the reference layout shows it.
    fn launcher(&self) -> String {
        page_text(self.system.shell())
    }

    /// What `terse run --shell --dump` prints.
    fn dump(&self) -> String {
        let mut out = Self::screen(self.system.shell());
        let dev = self.system.shell().machines().unwrap();
        for (h, child) in self.system.children() {
            let slot = dev.slot(h);
            let r = slot.rect;
            if r.w == 0 || r.h == 0 {
                continue;
            }
            out += &format!("-- machine {h} {} {} {} {} {} {} --\n", state::name(slot.state), r.x, r.y, r.w, r.h, child.title);
            if let Some(m) = &child.machine {
                out += &Self::screen(m);
            }
        }
        out
    }

    fn hashes(&self) -> Vec<String> {
        let mut out = vec![to_hex(&self.system.shell().state_hash())];
        for (h, c) in self.system.children() {
            out.push(format!("{h} {}", c.machine.as_ref().map_or("-".into(), |m| to_hex(&m.state_hash()))));
        }
        out
    }
}

/// The text of a machine's page lines, anchors as `[text]{id}`.
fn page_text(m: &Machine) -> String {
    let page = m.page().unwrap();
    let Some(doc) = page.document() else { return String::new() };
    let layout = reference_layout(doc, page.region().w.max(1));
    let mut out = String::new();
    for line in &layout.lines {
        for span in &line.spans {
            match span.style.anchor {
                0 => out += &span.text,
                id => out += &format!("[{}]{{{id}}}", span.text),
            }
        }
        out.push('\n');
    }
    out
}

fn golden(name: &str, got: &str) {
    common::golden(&format!("shell/expected/{name}.txt"), got);
}

/// From the launcher at start: reader (handle 1), then sparks, fetch and
/// hello, each started from the launcher after the home key (the cursor is
/// then on the focused machine's line, below the four programs).
const FOUR: &str = "\
2 press enter
10 home
11 press up
12 press up
13 press up
14 press enter
20 home
21 press up
22 press up
23 press up
24 press enter
30 home
31 press up
32 press up
33 press up
34 press enter
";

#[test]
fn launcher_at_start() {
    let mut s = Sys::new("", Setup::default());
    s.run(5);
    assert_eq!(s.system.focus(), 0);
    assert_eq!(s.system.children().count(), 0);
    let launcher = s.launcher();
    assert!(launcher.starts_with("PROGRAMS\n[reader]{100}\n[sparks]{101}\n[fetch]{102}\n[hello]{103}\n"), "{launcher}");
    let page = s.system.shell().page().unwrap().region();
    assert_eq!((page.x, page.y, page.w), (160, 80, 320), "centred while there are no machines");
    assert!(page.h < 340, "as high as the launcher's text: {}", page.h);
    golden("launcher", &format!("{}-- launcher --\n{launcher}", s.dump()));
}

#[test]
fn one_machine_fills_the_area() {
    let mut s = Sys::new("2 press enter\n", Setup::default());
    s.run(3);
    assert_eq!(s.state(1), state::LOADING, "the start in frame 2");
    s.run(1);
    assert_eq!(s.state(1), state::RUNNING, "loaded at the start of the frame after the start");
    assert_eq!(s.system.focus(), 1);
    s.run(30);
    // 634 x 454 (the area less the gaps) holds a 605 x 453 picture, at the top.
    assert_eq!(s.rect(1), (17, 23, 605, 454));
    assert_eq!(picture(s.system.shell().machines().unwrap().slot(1).rect).map(|p| (p.x, p.y, p.w, p.h)), Some((17, 23, 605, 453)));
    assert!(s.bar().starts_with(" ⌂  1 reader"), "{}", s.bar());
    assert_eq!(s.system.shell().page().unwrap().region().w, 0, "no launcher while a machine has the focus");
    golden("one", &s.dump());
}

#[test]
fn two_machines_master_and_stack() {
    let mut s = Sys::new("2 press enter\n10 home\n11 press up\n12 press enter\n", Setup::default());
    s.run(40);
    assert_eq!((s.state(1), s.state(2)), (state::RUNNING, state::RUNNING));
    assert_eq!(s.system.child(2).unwrap().title, "hello");
    assert_eq!(s.system.focus(), 2);
    // The left half and the right half, each picture at the top.
    assert_eq!(s.rect(1), (3, 23, 314, 235));
    assert_eq!(s.rect(2), (323, 23, 314, 235));
    golden("two", &s.dump());
}

#[test]
fn four_machines_and_the_layouts() {
    let mut s = Sys::new(&format!("{FOUR}50 home\n51 press l\n60 press l\n70 press l\n"), Setup::default());
    s.run(45);
    let titles: Vec<&str> = s.system.children().map(|(_, c)| c.title.as_str()).collect();
    assert_eq!(titles, ["reader", "sparks", "fetch", "hello"]);
    assert!((1..=4).all(|h| s.state(h) == state::RUNNING));
    // Master and stack: one on the left, three on the right.
    let master = [s.rect(1), s.rect(2), s.rect(3), s.rect(4)];
    assert_eq!(master, [(3, 23, 314, 235), (382, 23, 196, 147), (382, 176, 196, 147), (381, 329, 197, 148)]);
    golden("four-master", &s.dump());
    // Command mode: the launcher on the left, the tiles beside it; L at
    // frame 51: the grid (2 x 2).
    s.run(10);
    assert_eq!(s.system.focus(), 0);
    let launcher = s.launcher();
    assert!(launcher.contains("MACHINES\n[1  reader]{1}[  running"), "{launcher}");
    assert!(launcher.contains("[4  hello]{4}"), "{launcher}");
    let grid = [s.rect(1), s.rect(2), s.rect(3), s.rect(4)];
    assert_eq!(grid, [(203, 23, 214, 160), (423, 23, 214, 160), (203, 253, 214, 160), (423, 253, 214, 160)]);
    // L: zoom (the selected machine, 4, alone).
    s.run(10);
    assert_eq!(s.rect(4), (203, 23, 434, 325));
    assert_eq!((s.rect(1).2, s.rect(2).2, s.rect(3).2), (0, 0, 0), "the others are hidden");
    assert!((1..=4).all(|h| s.state(h) == state::RUNNING), "hidden machines run on");
    // L: master and stack again.
    s.run(10);
    assert_eq!(s.rect(1), (203, 23, 214, 160));
    // Esc: back to machine 4, still zoomed, over the whole area.
    let mut s = Sys::new(&format!("{FOUR}50 home\n51 press z\n53 press esc\n"), Setup::default());
    s.run(60);
    assert_eq!(s.system.focus(), 4);
    assert_eq!(s.rect(4), (17, 23, 605, 454));
    golden("four-zoom", &s.dump());
}

#[test]
fn typed_text_reaches_only_the_focused_machine() {
    // Two hellos; text to the second, then home, more text, then Esc
    // back to the second machine and more text.
    let script = "\
2 press down
3 press down
4 press down
5 press enter
10 text one
20 home
21 press up
22 press enter
30 text two
40 home
41 text not for anyone
42 press 1
43 press esc
50 text three
";
    let mut s = Sys::new(script, Setup::default());
    s.run(60);
    let first = Sys::screen(s.machine(1));
    let second = Sys::screen(s.machine(2));
    assert!(first.contains("> one") && first.contains("three"), "{first}");
    assert!(!first.contains("two") && !first.contains("anyone"), "{first}");
    assert!(second.contains("> two"), "{second}");
    assert!(!second.contains("one") && !second.contains("three") && !second.contains("anyone"), "{second}");
}

#[test]
fn home_key_never_reaches_a_machine() {
    // F12 as a key and as the home key: hello sees neither (it would show
    // nothing for a key, so the focus is checked: the shell has it, and
    // hello's queue never held a home event).
    let mut s = Sys::new("2 press down\n3 press down\n4 press down\n5 press enter\n20 home\n", Setup::default());
    s.run(15);
    assert_eq!(s.system.focus(), 1);
    s.run(6);
    assert_eq!(s.system.focus(), 0, "the home key gives the focus to the shell");
    assert!(s.machine(1).system().queue().all(|e| e.kind != Event::HOME));
    assert_eq!(s.launcher().lines().filter(|l| l.contains("hello")).count(), 2, "the launcher is shown");
}

#[test]
fn a_click_in_a_tile_reaches_the_reader_mapped() {
    // The reader alone: its picture is at (17, 23), 605 x 453. Its first
    // entry's title is at reader units y 51-70 (8 + 20 + 15 + 8 down,
    // reader.c), x from 32.
    let rx = 50u32;
    let ry = 60u32;
    let x = 17 + rx * 605 / 640;
    let y = 23 + ry * 453 / 480 + 1;
    let script = format!("2 press enter\n40 click {x} {y}\n");
    let mut s = Sys::new(&script, Setup { article: true, ..Setup::default() });
    s.run(30);
    let list = page_text(s.machine(1));
    assert!(list.contains("[On tiles]{1}"), "{list}");
    s.run(20);
    let text = page_text(s.machine(1));
    assert!(text.starts_with("On tiles\n"), "the click opened the entry: {text}");
    assert!(text.contains("Tiles are machines."), "{text}");
}

#[test]
fn a_click_focuses_a_tile_and_the_bar() {
    // Two machines (reader, hello: hello focused); a click in the
    // reader's tile focuses it; one on hello's bar segment focuses hello.
    let script = "2 press enter\n10 home\n11 press up\n12 press enter\n30 click 100 100\n40 click 330 8\n";
    let mut s = Sys::new(script, Setup::default());
    s.run(31);
    assert_eq!(s.system.focus(), 1);
    s.run(10);
    assert_eq!(s.system.focus(), 2);
}

#[test]
fn pause_and_resume() {
    let script = "2 press down\n3 press down\n4 press down\n5 press enter\n20 home\n21 press p\n50 press p\n51 press esc\n";
    let mut s = Sys::new(script, Setup::default());
    s.run(23);
    assert_eq!(s.state(1), state::PAUSED);
    assert!(s.bar().contains("1‖hello"), "{}", s.bar());
    let frame = s.machine(1).frame();
    s.run(25);
    assert_eq!(s.machine(1).frame(), frame, "a paused machine runs no frames");
    s.run(5);
    assert_eq!(s.state(1), state::RUNNING);
    assert_eq!(s.system.focus(), 1);
    assert!(s.machine(1).frame() > frame);
}

#[test]
fn stop() {
    let mut s = Sys::new("2 press enter\n10 home\n11 press up\n12 press enter\n20 home\n21 press x\n", Setup::default());
    s.run(25);
    assert_eq!(s.state(2), state::NONE);
    assert!(s.system.child(2).is_none());
    assert_eq!(s.state(1), state::RUNNING);
    assert!(!s.bar().contains("hello"), "{}", s.bar());
    // The one left fills the area again.
    assert_eq!(s.rect(1), (203, 23, 434, 325));
}

#[test]
fn exit_and_fault_are_reported() {
    // hello exits on Esc; the crash image (the fifth program) faults at once.
    let script = "\
2 press down
3 press down
4 press down
5 press enter
10 press esc
20 press up
21 press enter
";
    let mut s = Sys::new(script, Setup { crash: true, ..Setup::default() });
    s.run(15);
    assert_eq!(s.state(1), state::EXITED);
    assert_eq!(s.system.focus(), 0, "the shell has the focus when its machine exits");
    assert!(s.bar().contains("1■hello"), "{}", s.bar());
    assert!(s.launcher().contains("exited 0"), "{}", s.launcher());
    s.run(10);
    assert_eq!(s.state(2), state::FAULTED);
    let slot = *s.system.shell().machines().unwrap().slot(2);
    assert_eq!((slot.fault_kind, slot.fault_pc), (4, 0x0001_0014));
    assert!(s.bar().contains("2!crash"), "{}", s.bar());
    assert!(s.launcher().contains("load at pc 00010014"), "{}", s.launcher());
    golden("exit-fault", &format!("{}-- launcher --\n{}", s.dump(), s.launcher()));
}

#[test]
fn a_start_that_fails() {
    // The fifth program's ID is in no store: its title is unknown, and
    // starting it fails.
    let script = "2 press down\n3 press down\n4 press down\n5 press down\n6 press enter\n";
    let mut s = Sys::new(script, Setup { unknown: true, ..Setup::default() });
    s.run(5);
    assert!(s.launcher().contains("[?]{104}"), "{}", s.launcher());
    s.run(10);
    assert_eq!(s.state(1), state::FAILED);
    assert!(s.system.child(1).unwrap().machine.is_none());
    assert_eq!(s.system.focus(), 0);
    assert!(Sys::screen(s.system.shell()).contains("1 ?: could not start"), "{}", s.dump());
    assert!(s.bar().contains("1✗?"), "{}", s.bar());
}

#[test]
fn the_same_script_gives_the_same_hashes() {
    let script = format!("{FOUR}40 text abc\n50 home\n51 press l\n52 press 2\n53 press esc\n54 press enter\n");
    let mut a = Sys::new(&script, Setup::default());
    let mut b = Sys::new(&script, Setup::default());
    a.run(120);
    b.run(120);
    assert_eq!(a.hashes(), b.hashes());
    assert_eq!(a.hashes().len(), 5);
    // Each machine's hash is its own: hello with the same input alone.
    let mut alone = Sys::new("2 press down\n3 press down\n4 press down\n5 press enter\n", Setup::default());
    alone.run(10);
    assert_ne!(a.hashes()[4], format!("4 {}", to_hex(&alone.machine(1).state_hash())));
}

/// `terse run --shell --dump` prints what the library run gives.
#[test]
fn cli_dump_matches() {
    let dir = tempfile::tempdir().unwrap();
    let events = dir.path().join("four.events");
    std::fs::write(&events, FOUR).unwrap();
    let out = common::terse_command()
        .args(["run", "--shell", "--frames", "45", "--dump", "--hash-all", "--events"])
        .arg(&events)
        .arg("--store")
        .arg(dir.path().join("store"))
        .arg("--config")
        .arg(dir.path().join("config"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let mut s = Sys::new(FOUR, Setup::default());
    s.run(45);
    let want = format!("{}\n{}", s.hashes().join("\n"), s.dump());
    assert_eq!(String::from_utf8_lossy(&out.stdout), want);
}

/// The shell's frames, starting four machines and changing the layout.
#[test]
fn budget() {
    let mut s = Sys::new(&format!("{FOUR}50 home\n51 press l\n60 press l\n70 press l\n80 press esc\n"), Setup::default());
    s.run(200);
    let stats = FrameStats::new(&s.shell_units);
    println!("shell: {}", stats.describe());
    assert!(stats.busiest < terse_core::BUDGET_PER_FRAME as u64 / 8, "{}", stats.describe());
    assert!(stats.median < terse_core::BUDGET_PER_FRAME as u64 / 100, "{}", stats.describe());
}

#[test]
fn image_is_up_to_date() {
    common::check_up_to_date(SHELL, &["shell/shell.c"], true, "shell", 2, &[(2, 0), (3, 0), (4, 0), (7, 0), (8, 0)]);
    assert_eq!(SHELL, builtin::SHELL, "the shell built into the hosts");
    assert!(Image::parse(SHELL).unwrap().rom.len() < 131_072 / 8);
}

