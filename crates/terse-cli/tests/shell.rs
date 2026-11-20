//! programs/shell/shell.terse: the shell and the machines it starts (SPEC
//! 12), run as `terse run --shell` runs them, with the built-in programs in a
//! temporary store. Golden screens are in programs/shell/expected/ (the
//! shell's screen, then each placed machine's header line and screen, as
//! `terse run --shell --dump` prints them; UPDATE_GOLDEN=1 rewrites them).

mod common;

use common::FrameStats;
use terse_core::machines::{picture, state};
use terse_core::page::reference_layout;
use terse_core::{Event, Image, Machine, ProgramId, Script, System};
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
    Image::build("crash", 1, &[terse_core::DeviceReq::new(2, 0)], &rom)
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
    /// The data directory (named records), unless the setup gave one.
    _data: Option<tempfile::TempDir>,
    /// Units the shell used in each frame.
    shell_units: Vec<u64>,
    /// The same session on the device bus (common/twin.rs).
    twin: Option<common::twin::Twin>,
}

/// What a run has: extra built-in images after the four, an ID in no
/// store after them, an article for the reader.
#[derive(Default, Clone)]
struct Setup {
    crash: bool,
    unknown: bool,
    article: bool,
    /// Programs published in a followed feed: (name, image, downloaded).
    programs: Vec<(String, Vec<u8>, bool)>,
    /// The data directory for named records (default: a new one).
    data: Option<std::path::PathBuf>,
}

impl Sys {
    fn new(script: &str, setup: Setup) -> Self {
        let store = tempfile::tempdir().unwrap();
        let mut node = Node::new(BlobStore::open(store.path(), None).unwrap(), None);
        if setup.article || !setup.programs.is_empty() {
            node.set_identity(alice());
            node.set_subscriptions(Subscriptions::in_memory([alice().feed_key()]));
        }
        if setup.article {
            let draft = Draft { data: b"Tiles are machines.\n".to_vec(), title: "On tiles".into(), reply_to: None, time: 1_800_000_000 };
            assert!(matches!(node.publish_article(&draft), terse_net::Done::Published { .. }));
        }
        if !setup.programs.is_empty() {
            assert!(matches!(node.publish_profile("Alice", false, Some(String::new()), 1_800_000_000), terse_net::Done::Published { .. }));
        }
        for (name, image, downloaded) in &setup.programs {
            assert!(matches!(node.publish_program(image, name, 1_800_000_000), terse_net::Done::Published { .. }));
            if !downloaded {
                node.store().remove(&terse_net::blob_id(image)).unwrap();
            }
        }
        let data = match &setup.data {
            Some(_) => None,
            None => Some(tempfile::tempdir().unwrap()),
        };
        let data_dir = setup.data.clone().unwrap_or_else(|| data.as_ref().unwrap().path().to_path_buf());
        node.set_records(terse_net::Records::in_data_dir(&data_dir));
        let extra: Vec<Vec<u8>> = if setup.crash { vec![crash_image()] } else { Vec::new() };
        let mut ids = builtin::install(node.store(), &extra).unwrap();
        if setup.unknown {
            ids.push(UNKNOWN);
        }
        let shell = Image::parse(SHELL).unwrap();
        let identity = ProgramId { publisher: [0; 32], name: shell.title.clone() };
        let alice_node = setup.article || !setup.programs.is_empty();
        let twin = common::twin::Twin::start(common::twin::Setup {
            image: &shell,
            shell: Some(ids.clone()),
            identity: Some(identity.clone()),
            store: Some(store.path()),
            data: Some(&data_dir),
            subscriptions: if alice_node { vec![alice().feed_key()] } else { Vec::new() },
            seed: alice_node.then_some([11; 32]),
            consent: Consent::Deny,
            relays: Vec::new(),
            script,
            content: true,
            announced: alice_node,
        });
        let content = ContentService::start_with_consent(node, Consent::Deny);
        let mut system = System::new(&shell, true, ids);
        // The built-in shell's identity, as the hosts give it.
        system.set_shell_identity(Some(identity));
        let headless = HeadlessSystem::new(Some(content), Script::parse(script).unwrap());
        Sys { system, headless, _store: store, _data: data, shell_units: Vec::new(), twin }
    }

    fn run(&mut self, frames: u32) {
        for _ in 0..frames {
            let r = self.headless.frame(&mut self.system);
            if let Some(t) = &mut self.twin {
                t.follow(&self.system);
            }
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
/// then on the focused machine's line, below the six programs).
const FOUR: &str = "\
2 press enter
10 home
11 press up
11 press up
12 press up
13 press up
14 press up
15 press enter
20 home
21 press up
21 press up
22 press up
23 press up
24 press up
25 press enter
30 home
31 press up
31 press up
32 press up
33 press up
34 press up
35 press enter
";

#[test]
fn launcher_at_start() {
    let mut s = Sys::new("", Setup::default());
    s.run(5);
    assert_eq!(s.system.focus(), 0);
    assert_eq!(s.system.children().count(), 0);
    let launcher = s.launcher();
    assert!(
        launcher.starts_with("PROGRAMS\n[Manual]{301}\n[reader]{100}\n[sparks]{101}\n[fetch]{102}\n[hello]{103}\n[sketch]{104}\n[holo]{105}\n"),
        "{launcher}"
    );
    let page = s.system.shell().page().unwrap().region();
    assert_eq!((page.x, page.y, page.w), (160, 80, 320), "centred while there are no machines");
    assert!(page.h < 340, "as high as the launcher's text: {}", page.h);
    golden("launcher", &format!("{}-- launcher --\n{launcher}", s.dump()));
}

/// The launcher's Manual entry (above the programs; the cursor starts on
/// the first program, so Up reaches it) starts the reader with the
/// manual's index as its argument, which the reader shows at once.
#[test]
fn manual_entry_opens_the_manual() {
    let mut s = Sys::new("2 press up\n3 press enter\n", Setup::default());
    s.run(30);
    assert_eq!(s.system.focus(), 1);
    assert_eq!(s.system.child(1).unwrap().title, "reader");
    let text = page_text(s.machine(1));
    assert!(text.starts_with("The Terse manual\n"), "{text}");
    // A click on the entry does the same.
    let mut c = Sys::new("", Setup::default());
    c.run(5);
    let page = c.system.shell().page().unwrap();
    let layout = reference_layout(page.document().unwrap(), page.region().w);
    let line = layout.lines.iter().find(|l| l.spans.iter().any(|s| s.style.anchor == 301)).unwrap();
    let r = page.region();
    let (x, y) = (r.x as u32 + 20, r.y as u32 + line.y + 4);
    let mut c = Sys::new(&format!("8 click {x} {y}\n"), Setup::default());
    c.run(30);
    assert!(page_text(c.machine(1)).starts_with("The Terse manual\n"), "clicked at {x} {y}");
}

/// The shell holds the ID of the manual's current index (its Manual
/// entry): a change of the manual without a rebuild fails here.
#[test]
fn shell_knows_the_manual_index() {
    let id = terse_net::blob_id(builtin::MANUAL[0].2);
    let rom = &Image::parse(SHELL).unwrap().rom;
    assert!(rom.windows(32).any(|w| w == id), "programs/shell/shell.terse is stale: run programs/shell/build.sh");
    let header = include_str!("../../../programs/shell/manual_id.h");
    let bytes: String = id.iter().map(|b| format!("0x{b:02x}")).collect::<Vec<_>>().join(", ");
    assert!(header.contains(&bytes), "programs/shell/manual_id.h is stale: run programs/shell/build.sh");
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
    let mut s = Sys::new("2 press enter\n10 home\n11 press up\n11 press up\n12 press up\n13 press enter\n", Setup::default());
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
    let mut s = Sys::new(&format!("{FOUR}50 home\n51 press l\n60 press l\n65 press l\n70 press l\n"), Setup::default());
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
    // L: the gallery: no tiles, the machines are panels in the 3D list.
    s.run(10);
    assert!((1..=4).all(|h| s.rect(h).2 == 0));
    assert_eq!(terse_core::vector3::screens_in(s.system.shell().vector3().unwrap().list()), 0b1111);
    // L: zoom (the selected machine, 4, alone).
    s.run(5);
    assert_eq!(s.rect(4), (203, 23, 434, 325));
    assert!(s.system.shell().vector3().unwrap().list().is_empty(), "the ring is gone");
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
21 press up
22 press up
23 press enter
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
    let mut s = Sys::new(&script, Setup { article: true, ..Default::default() });
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
    let script = "2 press enter\n10 home\n11 press up\n11 press up\n12 press up\n13 press enter\n30 click 100 100\n40 click 330 8\n";
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
    let mut s = Sys::new("2 press enter\n10 home\n11 press up\n11 press up\n12 press up\n13 press enter\n20 home\n21 press x\n", Setup::default());
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
    // hello exits on Esc; the crash image (the sixth program) faults at once.
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
    let slot = s.system.shell().machines().unwrap().slot(2).clone();
    assert_eq!((slot.fault_kind, slot.fault_pc), (4, 0x0001_0014));
    assert!(s.bar().contains("2!crash"), "{}\n{}", s.bar(), s.launcher());
    assert!(s.launcher().contains("load at pc 00010014"), "{}", s.launcher());
    golden("exit-fault", &format!("{}-- launcher --\n{}", s.dump(), s.launcher()));
}

#[test]
fn a_start_that_fails() {
    // An extra program's ID is in no store: its title is unknown, and
    // starting it fails.
    let script = "2 press down\n3 press down\n4 press down\n5 press down\n6 press down\n6 press down\n7 press enter\n";
    let mut s = Sys::new(script, Setup { unknown: true, ..Setup::default() });
    s.run(5);
    assert!(s.launcher().contains("[?]{106}"), "{}", s.launcher());
    s.run(10);
    assert_eq!(s.state(1), state::FAILED);
    assert!(s.system.child(1).unwrap().machine.is_none());
    assert_eq!(s.system.focus(), 0);
    assert!(Sys::screen(s.system.shell()).contains("1 ?\n"), "{}", s.dump());
    assert!(Sys::screen(s.system.shell()).contains("not found\n"), "{}", s.dump());
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
        .arg("--data")
        .arg(dir.path().join("data"))
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
    common::check_up_to_date(SHELL, &["shell/shell.c"], true, "shell", 3, &[(2, 0), (3, 0), (4, 0), (7, 0), (8, 0), (9, 0)]);
    assert_eq!(SHELL, builtin::SHELL, "the shell built into the hosts");
    assert!(Image::parse(SHELL).unwrap().rom.len() < 20 * 1024);
}


/// Programs of a followed feed: "Broken" needs a device no host provides,
/// "Greeter" is hello under another title (downloaded), "Racer" is sparks,
/// whose image the store does not hold (and there is no relay).
fn feed_programs() -> Vec<(String, Vec<u8>, bool)> {
    let hello = Image::parse(builtin::PROGRAMS[3]).unwrap();
    let greeter = Image::build("greeter", hello.ram_pages, &hello.devices, &hello.rom);
    let mut broken = Image::build("broken", 1, &[terse_core::DeviceReq::new(2, 0)], &[0x6F, 0, 0, 0]);
    broken[32] = 5;
    let sparks = Image::parse(builtin::PROGRAMS[1]).unwrap();
    let racer = Image::build("racer", sparks.ram_pages, &sparks.devices, &sparks.rom);
    vec![
        ("Greeter".into(), greeter, true),
        ("Racer".into(), racer, false),
        ("Broken".into(), broken, true),
    ]
}

/// The launcher lists the built-ins, then the programs of the followed
/// feeds (op 0Dh) with the publisher's name and short key and a mark when
/// the image is not downloaded; Enter starts one by its identity; failed
/// starts say why.
#[test]
fn programs_from_feeds() {
    let script = "\
2 press down
3 press down
4 press down
5 press down
6 press down
7 press down
7 press down
8 press enter
15 home
16 press up
17 press up
18 press up
19 press enter
25 press up
26 press up
27 press enter
";
    let mut s = Sys::new(script, Setup { programs: feed_programs(), ..Default::default() });
    s.run(5);
    let launcher = s.launcher();
    let key = to_hex(&alice().feed_key());
    let short = format!("{}…{}", &key[..6], &key[58..]);
    assert!(
        launcher.contains(&format!(
            "FROM YOUR FEEDS\n[Broken]{{200}}\n[Alice {short}]{{200}}\n[Greeter]{{201}}\n[Alice {short}]{{201}}\n[Racer]{{202}}[ ↓]{{202}}\n"
        )),
        "{launcher}"
    );
    golden("feed-programs", &format!("{}-- launcher --\n{launcher}", s.dump()));
    // Greeter: started by identity; its title is its image's.
    s.run(12);
    assert_eq!(s.state(1), state::RUNNING);
    let child = s.system.child(1).unwrap();
    assert_eq!(child.identity, Some(ProgramId { publisher: alice().feed_key(), name: b"Greeter".to_vec() }));
    assert_eq!(child.title, "greeter");
    assert!(s.bar().contains("1 greeter"), "{}", s.bar());
    // Broken: the image lists device 5, which no host provides: failed, 3.
    s.run(8);
    assert_eq!(s.state(2), state::FAILED);
    assert_eq!(s.system.shell().machines().unwrap().slot(2).failure, 3);
    assert!(s.system.child(2).unwrap().machine.is_none(), "it never starts");
    // Racer: the entry is held, its image is not, and no relay has it.
    s.run(5);
    assert_eq!(s.state(3), state::FAILED);
    assert_eq!(s.system.shell().machines().unwrap().slot(3).failure, 1);
    let launcher = s.launcher();
    assert!(launcher.contains("[2  Broken]{2}[  failed]{2}\n[needs a device this host]{2}\n[does not have]{2}"), "{launcher}");
    assert!(launcher.contains("[3  Racer]{3}[  failed]{3}\n[not found]{3}"), "{launcher}");
    let screen = Sys::screen(s.system.shell());
    assert!(screen.contains("2 Broken\n") && screen.contains("needs a device this host"), "{screen}");
    assert!(screen.contains("3 Racer\n") && screen.contains("not found\n"), "{screen}");
    golden("feed-programs-started", &format!("{}-- launcher --\n{launcher}", s.dump()));
}

/// The shell keeps its layout in the named record "layout" (SPEC 9.6): L
/// in one run, and the next run tiles four machines as a grid.
#[test]
fn the_layout_is_remembered() {
    let data = tempfile::tempdir().unwrap();
    let mut s = Sys::new("2 press l\n", Setup { data: Some(data.path().to_path_buf()), ..Default::default() });
    s.run(5);
    let owner = terse_net::Owner::Program { publisher: [0; 32], name: b"shell".to_vec() };
    assert_eq!(terse_net::Records::in_data_dir(data.path()).load(&owner, b"layout").unwrap(), Some(vec![2, 2]));
    let mut s = Sys::new(FOUR, Setup { data: Some(data.path().to_path_buf()), ..Default::default() });
    s.run(45);
    assert_eq!(s.rect(3).0, s.rect(1).0, "a grid: machine 3 under machine 1");
    let mut fresh = Sys::new(FOUR, Setup::default());
    fresh.run(45);
    assert_ne!(fresh.rect(3).0, fresh.rect(1).0, "master and stack without the record");
}

// --- The gallery (SPEC 12.2 face, 13.3 Screen) ---------------------------

/// Starts `n` built-ins from the launcher (reader, sparks, fetch, hello,
/// sketch in turn), each focused as it starts, then the home key and L
/// twice: master, grid, the gallery. Returns the script and the first
/// frame after it. The cursor starts on the first built-in; after the
/// home key, Up to the top is the Manual entry, the built-ins below it.
fn gallery_script(n: usize) -> (String, u32) {
    let mut out = String::new();
    let mut f = 2;
    for k in 0..n {
        let mut down = 0;
        if k > 0 {
            out += &format!("{f} home\n");
            out += &format!("{} press up\n", f + 1).repeat(14);
            down = k + 1;
        }
        out += &format!("{} press down\n", f + 2).repeat(down);
        out += &format!("{} press enter\n", f + 3);
        f += 10;
    }
    out += &format!("{f} home\n{} press l\n{} press l\n", f + 1, f + 2);
    (out, f + 3)
}

impl Sys {
    /// Runs until the shell's frame counter is `frame`.
    fn run_until(&mut self, frame: u32) {
        let now = self.system.shell().frame();
        self.run(frame.saturating_sub(now));
    }

    /// The view of the shell's 3D scene (SPEC 12.2 face).
    fn view(&self) -> terse_core::machines::View {
        *self.system.shell().machines().unwrap().view()
    }

    /// The view's progress after the shell's last frame.
    fn progress(&self) -> u16 {
        self.view().progress(self.system.shell().frame() - 1)
    }

    /// The dump, then the 3D list, the view and the flat pictures.
    fn gallery_dump(&self) -> String {
        let v = self.view();
        let r = v.rect;
        let mut out = self.dump();
        out += &self.system.shell().vector3().unwrap().dump();
        out += &format!(
            "-- view: handle {} {} rect {} {} {} {} from {} start {} frames {} progress {} --\n",
            v.handle,
            if v.toward { "toward" } else { "back" },
            r.x,
            r.y,
            r.w,
            r.h,
            v.from,
            v.start,
            v.duration,
            self.progress()
        );
        for (h, p) in self.system.pictures() {
            out += &format!("-- flat picture {h}: {} {} {} {} --\n", p.x, p.y, p.w, p.h);
        }
        out
    }

    /// The panels as seen from the reference eye.
    fn panels(&self) -> Vec<terse_core::vector3::Panel> {
        self.system.scene((0.0, 0.0)).map_or(Vec::new(), |s| s.panels)
    }

    fn screens(&self) -> u8 {
        terse_core::vector3::screens_in(self.system.shell().vector3().unwrap().list())
    }
}

/// A point on the glass (whole units) over the front of panel `h` from the
/// reference eye, where the pointer meets that panel and no other.
fn point_on(panels: &[terse_core::vector3::Panel], h: u8) -> (u16, u16) {
    use terse_core::vector3::{EYE_X, EYE_Y, panel_hit, to_glass};
    let p = panels.iter().find(|p| p.handle == h).unwrap_or_else(|| panic!("no panel {h}: {panels:?}"));
    let g = p.corners.map(|c| to_glass(c, (EYE_X, EYE_Y)));
    // Towards the centre from each corner in turn, until a point hits it.
    for k in 0..4 {
        for t in [0.5, 0.3, 0.7] {
            let x = g[k].0 + ((g[0].0 + g[1].0 + g[2].0 + g[3].0) / 4.0 - g[k].0) * t;
            let y = g[k].1 + ((g[0].1 + g[1].1 + g[2].1 + g[3].1) / 4.0 - g[k].1) * t;
            let (x, y) = (x.floor() as u16, y.floor() as u16);
            if let Some((hit, Some(_))) = panel_hit(panels, (0.0, 0.0), x as f64 + 0.5, y as f64 + 0.5)
                && hit == h
            {
                return (x, y);
            }
        }
    }
    panic!("no point on panel {h}");
}

fn close(a: [f64; 3], b: [f64; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < 1e-6)
}

/// The ring with 1, 3 and 5 machines: every machine a panel (none placed
/// flat), the selected one (the last started) at the front on the glass,
/// 304 units wide about (450, 250).
#[test]
fn gallery_rings() {
    for n in [1usize, 3, 5] {
        let (script, at) = gallery_script(n);
        let mut s = Sys::new(&script, Setup::default());
        s.run_until(at + 3);
        assert_eq!(s.system.focus(), 0);
        assert!((1..=n as u8).all(|h| s.state(h) == state::RUNNING && s.rect(h).2 == 0));
        assert_eq!(s.screens(), (1u16 << n) as u8 - 1);
        assert!(s.system.pictures().is_empty(), "nothing flat");
        let panels = s.panels();
        let front = panels.iter().find(|p| p.handle == n as u8).unwrap();
        assert!(close(front.corners[0], [298.0, 136.0, 0.0]) && close(front.corners[2], [602.0, 364.0, 0.0]), "{:?}", front.corners);
        assert!(front.front);
        assert_eq!(s.view(), Default::default());
        golden(&format!("gallery-ring-{n}"), &s.gallery_dump());
    }
}

/// Left in command mode turns the ring over 20 frames to the previous
/// machine: part way, then at rest with it at the front.
#[test]
fn gallery_turning() {
    let (script, at) = gallery_script(3);
    let t = at + 5;
    let mut s = Sys::new(&format!("{script}{t} press left\n"), Setup::default());
    s.run_until(t);
    let before = s.system.shell().vector3().unwrap().list().to_vec();
    s.run_until(t + 10);
    let mid = s.system.shell().vector3().unwrap().list().to_vec();
    assert_ne!(before, mid);
    let p = s.panels();
    assert!(!p.iter().any(|p| close(p.corners[0], [298.0, 136.0, 0.0])), "nothing at the front part way");
    golden("gallery-turning", &s.gallery_dump());
    s.run_until(t + 25);
    let front = s.panels().into_iter().find(|p| p.handle == 2).unwrap();
    assert!(close(front.corners[0], [298.0, 136.0, 0.0]), "{:?}", front.corners);
    golden("gallery-turned", &s.gallery_dump());
    // At rest the list is not submitted again: the same bytes.
    let rest = s.system.shell().vector3().unwrap().generation();
    s.run(10);
    assert_eq!(s.system.shell().vector3().unwrap().generation(), rest);
}

/// Enter faces the selected machine: the view moves for 20 frames, then
/// shows it flat over the zoom tile's picture and it has the keyboard;
/// the home key moves the view back.
#[test]
fn gallery_face_and_unface() {
    let (script, at) = gallery_script(3);
    let (e, u) = (at + 5, at + 40);
    let mut s = Sys::new(&format!("{script}{e} press enter\n{} text hi\n{u} home\n", e + 2), Setup::default());
    s.run_until(e + 1);
    assert_eq!(s.system.focus(), 3);
    let v = s.view();
    assert_eq!((v.handle, v.toward, v.start, v.duration), (3, true, e, 20));
    assert_eq!((v.rect.x, v.rect.y, v.rect.w, v.rect.h), (17, 23, 605, 454));
    assert!(s.system.scene((0.0, 0.0)).unwrap().moving);
    s.run_until(e + 11);
    assert_eq!(s.progress(), 2048);
    s.run_until(e + 21);
    assert_eq!(s.progress(), 4096);
    let scene = s.system.scene((0.0, 0.0)).unwrap();
    assert!(!scene.moving && scene.faced == 3);
    assert!(scene.panels.iter().all(|p| p.handle != 3), "shown flat, not as a panel");
    let pictures: Vec<_> = s.system.pictures().into_iter().map(|(h, p)| (h, p.x, p.y, p.w, p.h)).collect();
    assert_eq!(pictures, [(3, 17, 23, 605, 453)]);
    assert!(Sys::screen(s.machine(3)).contains("hi"), "the keyboard went to it");
    golden("gallery-faced", &s.gallery_dump());
    s.run_until(u + 1);
    assert_eq!(s.system.focus(), 0);
    let v = s.view();
    assert_eq!((v.handle, v.toward, v.from, v.start), (3, false, 4096, u));
    assert!(s.system.pictures().is_empty());
    s.run_until(u + 21);
    assert_eq!(s.progress(), 0);
    let scene = s.system.scene((0.0, 0.0)).unwrap();
    assert!(!scene.moving && scene.faced == 0 && scene.panels.len() == 3);
    golden("gallery-unfaced", &s.gallery_dump());
}

/// A click on a side panel (seen from the reference eye) focuses that
/// machine and faces it; a click over a panel while the view moves is not
/// delivered.
#[test]
fn gallery_click_on_a_side_panel() {
    let (script, at) = gallery_script(5);
    let c = at + 5;
    let mut first = Sys::new(&script, Setup::default());
    first.run_until(c);
    let panels = first.panels();
    let side = panels.iter().rev().find(|p| p.front && p.handle != 5).map(|p| p.handle).unwrap();
    let (x, y) = point_on(&panels, side);
    let mut s = Sys::new(&format!("{script}{c} click {x} {y}\n"), Setup::default());
    s.run_until(c + 1);
    assert_eq!(s.system.focus(), side, "clicked at {x} {y}");
    assert_eq!(s.view().handle, side);
    s.run_until(c + 25);
    assert_eq!(s.system.pictures().iter().map(|p| p.0).collect::<Vec<_>>(), [side]);
    golden("gallery-clicked", &s.gallery_dump());

    // Enter faces machine 5; five frames on, a click on a side panel.
    let e = at + 5;
    let mut first = Sys::new(&format!("{script}{e} press enter\n"), Setup::default());
    first.run_until(e + 5);
    let scene = first.system.scene((0.0, 0.0)).unwrap();
    assert!(scene.moving);
    let side = scene.panels.iter().rev().find(|p| p.front && p.handle != 5).map(|p| p.handle).unwrap();
    let (x, y) = point_on(&scene.panels, side);
    let mut s = Sys::new(&format!("{script}{e} press enter\n{} click {x} {y}\n", e + 5), Setup::default());
    s.run_until(e + 7);
    assert_eq!(s.system.focus(), 5, "not delivered");
    assert_eq!(s.view().handle, 5);
}

/// Starting a program in the gallery puts it on the ring, turns to it and
/// faces it once it runs.
#[test]
fn gallery_start_a_program() {
    let (script, at) = gallery_script(2);
    // The cursor is on machine 2's line (7): up 4 is hello.
    let st = at + 5;
    let mut s = Sys::new(&format!("{script}{}{st} press enter\n", format!("{} press up\n", st - 1).repeat(4)), Setup::default());
    s.run_until(st + 1);
    assert_eq!(s.state(3), state::LOADING);
    assert_eq!(s.system.focus(), 3);
    assert_eq!(s.view().handle, 0, "no picture yet");
    s.run_until(st + 40);
    assert_eq!(s.system.child(3).unwrap().title, "hello");
    assert_eq!(s.screens(), 0b111);
    let v = s.view();
    assert_eq!((v.handle, v.toward), (3, true));
    assert_eq!(s.progress(), 4096);
    assert_eq!(s.system.pictures().iter().map(|p| p.0).collect::<Vec<_>>(), [3]);
    golden("gallery-started", &s.gallery_dump());
}

/// The shell's frames in the gallery with five machines while the ring
/// turns again and again, and machines are faced and unfaced.
#[test]
fn gallery_budget() {
    let (mut script, at) = gallery_script(5);
    let mut f = at + 5;
    for k in 0..8 {
        script += &format!("{f} press {}\n", if k % 3 == 2 { "right" } else { "left" });
        f += 12;
    }
    script += &format!("{f} press enter\n{} home\n", f + 30);
    let mut s = Sys::new(&script, Setup::default());
    s.run(f + 60);
    let all = FrameStats::new(&s.shell_units);
    let gallery = FrameStats::new(&s.shell_units[at as usize..]);
    println!("shell, whole run: {}", all.describe());
    println!("shell, in the gallery: {}", gallery.describe());
    // The busiest frames turn the ring and lay the launcher out again
    // (as Tab does in any layout) and build the 3D list; while the ring
    // turns only its angles are written again.
    assert!(all.busiest < terse_core::BUDGET_PER_FRAME as u64 / 4, "{}", all.describe());
    assert!(gallery.median < terse_core::BUDGET_PER_FRAME as u64 / 50, "{}", gallery.describe());
}
