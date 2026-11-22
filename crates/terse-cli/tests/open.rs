//! Open requests end to end (SPEC §system `open`, 12.2 `request` and `argument`,
//! 9.7 documents): the shell, the reader and Sketch run as `terse run
//! --shell` runs them, with temporary stores, records and (where a program
//! is looked up or a sketch travels between hosts) an ephemeral relay.
//! Golden screens are in programs/shell/expected/open-*.txt.

mod common;

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use terse_core::machines::state;
use terse_core::{Image, Machine, ProgramId, Script, System};
use terse_net::relay::{DEFAULT_TIMEOUT, RelayClient};
use terse_net::{BlobStore, Consent, ContentService, Draft, Entry, HeadlessSystem, Identity, Node, Subscriptions, blob_id, builtin, to_hex};

/// The author of the articles the reader shows (followed).
fn alice() -> Identity {
    Identity::from_seed([11; 32])
}

/// A publisher the host does not follow, whose program is on the relay.
fn bob() -> Identity {
    Identity::from_seed([12; 32])
}

/// hello under the title "greeter": Bob's program "Greeter", and the
/// unpublished image a link names by its ID.
fn greeter() -> Vec<u8> {
    let hello = Image::parse(builtin::PROGRAMS[3]).unwrap();
    Image::build("greeter", hello.ram_pages, &hello.devices, &hello.rom)
}

/// A sketch document of two strokes (programs/sketch/sketch.c's format).
fn drawing() -> Vec<u8> {
    let mut data = Vec::new();
    for (colour, points) in [(1u8, [(100u16, 100u16), (300, 200), (500, 120)]), (3, [(80, 400), (320, 300), (560, 420)])] {
        data.push(colour);
        data.extend_from_slice(&(points.len() as u16).to_le_bytes());
        for (x, y) in points {
            data.extend_from_slice(&x.to_le_bytes());
            data.extend_from_slice(&y.to_le_bytes());
        }
    }
    [&b"TDOC\0"[..], &[0; 32], &[6], b"sketch", &data].concat()
}

/// A sheet document (programs/sheet/sheet.c's format: the built-in
/// format "sheet", then the rows, cells separated by tabs).
fn sheet_document() -> Vec<u8> {
    [&b"TDOC\0"[..], &[0; 32], &[5], b"sheet", b"Lunch money\t12\nBus\t3\nTotal\t=SUM(B1:B2)\n"].concat()
}

const SECOND: &[u8] = b"# The second article\n\nOpened in a tile of its own.\n";

fn serve(relay: Arc<terse_relay::Relay>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || terse_relay::serve(listener, relay));
    url
}

/// A relay holding Bob's program "Greeter".
fn bobs_relay(dir: &std::path::Path) -> String {
    let relay = terse_relay::Relay::new(BlobStore::open(dir, None).unwrap(), 1024);
    let image = greeter();
    relay.blobs().put(&image).unwrap();
    let e = Entry::program(bob().feed_key(), 1, 1, &image, "Greeter").sign(&bob()).unwrap();
    assert_eq!(relay.publish(&e.bytes), 1);
    serve(Arc::new(relay))
}

/// The links of the first article, in order: Bob's program, the image by
/// its ID, the drawing, the second article.
fn links_article() -> String {
    format!(
        "Things to open:\n\n- [Greeter](terse:program/{}/Greeter)\n- [an image](terse:{})\n- [a drawing](terse:{})\n- [more](terse:{})\n",
        to_hex(&bob().feed_key()),
        to_hex(&blob_id(&greeter())),
        to_hex(&blob_id(&drawing())),
        to_hex(&blob_id(SECOND)),
    )
}

struct Sys {
    system: System,
    headless: HeadlessSystem,
    _dirs: Vec<tempfile::TempDir>,
    shell_units: Vec<u64>,
    /// The same session on the device bus (common/twin.rs).
    twin: Option<common::twin::Twin>,
}

struct Setup {
    /// A relay to connect to.
    relay: Option<String>,
    consent: Consent,
    /// The store, config and data directory to use (default: new ones).
    dirs: Option<(std::path::PathBuf, std::path::PathBuf)>,
    /// Publish alice's articles and drawing into the store.
    content: bool,
    /// More built-in images, after the five.
    extra: Vec<Vec<u8>>,
}

impl Default for Setup {
    fn default() -> Self {
        Setup { relay: None, consent: Consent::Deny, dirs: None, content: true, extra: Vec::new() }
    }
}

impl Sys {
    fn new(script: &str, setup: Setup) -> Self {
        let mut dirs = Vec::new();
        let (store_dir, data_dir) = setup.dirs.clone().unwrap_or_else(|| {
            let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
            let paths = (s.path().to_path_buf(), d.path().to_path_buf());
            dirs.push(s);
            dirs.push(d);
            paths
        });
        let mut node = Node::new(BlobStore::open(&store_dir, None).unwrap(), None);
        node.set_identity(alice());
        node.set_subscriptions(Subscriptions::in_memory([alice().feed_key()]));
        if setup.content {
            node.store().put(SECOND).unwrap();
            node.store().put(&greeter()).unwrap();
            node.store().put(&sheet_document()).unwrap();
            node.start_publish_document(1, &drawing(), "A drawing", 1_800_000_000);
            let draft = Draft { data: links_article().into_bytes(), title: "Links".into(), reply_to: None, time: 1_800_000_100 };
            assert!(matches!(node.publish_article(&draft), terse_net::Done::Published { seq: 2, .. }));
        }
        if let Some(url) = &setup.relay {
            node.add_relay(url, RelayClient::websocket(url, DEFAULT_TIMEOUT));
        }
        node.set_records(terse_net::Records::in_data_dir(&data_dir));
        let ids = builtin::install(node.store(), &setup.extra).unwrap();
        let shell = Image::parse(builtin::SHELL).unwrap();
        let identity = ProgramId { publisher: [0; 32], name: shell.title.clone() };
        // A session that publishes to a live relay hears its own entry
        // back when the relay sends it, a moment that differs from run to
        // run: no twin can follow it frame by frame.
        let publishes = setup.consent == Consent::Allow && setup.relay.is_some();
        let twin = common::twin::Twin::start(common::twin::Setup {
            image: &shell,
            shell: Some(ids.clone()),
            identity: Some(identity.clone()),
            store: Some(&store_dir),
            data: Some(&data_dir),
            subscriptions: vec![alice().feed_key()],
            seed: Some([11; 32]),
            consent: setup.consent,
            relays: setup.relay.iter().cloned().collect(),
            script,
            content: true,
            // (Catching up with a relay takes the feed events.)
            announced: setup.content && setup.relay.is_none(),
        })
        .filter(|_| !publishes);
        let mut content = ContentService::start_with_consent(node, setup.consent);
        if setup.relay.is_some() {
            content.wait_caught_up(Duration::from_secs(10)).unwrap();
        }
        let mut system = System::new(&shell, true, ids);
        system.set_shell_identity(Some(identity));
        let headless = HeadlessSystem::new(Some(content), Script::parse(script).unwrap());
        Sys { system, headless, _dirs: dirs, shell_units: Vec::new(), twin }
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

    fn title(&self, h: u8) -> String {
        self.system.child(h).map_or(String::new(), |c| c.title.clone())
    }

    fn bar(&self) -> String {
        self.system.shell().terminal().unwrap().text().lines().next().unwrap().trim_end().to_string()
    }

    fn screen(m: &Machine) -> String {
        m.terminal().map_or(String::new(), |t| t.text())
    }

    /// The shell's screen, then each placed machine's header and screen.
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
}

fn golden(name: &str, got: &str) {
    common::golden(&format!("shell/expected/open-{name}.txt"), got);
}

/// The reader (handle 1) opens the first article ("Links"), then Tab makes
/// link `n` current and Enter follows it.
fn follow(n: u32) -> String {
    let mut s = String::from("2 press enter\n20 press enter\n");
    for i in 0..n {
        s += &format!("{} press tab\n", 30 + i);
    }
    s += "40 press enter\n";
    s
}

/// The reader follows a link to Bob's program, which the host does not
/// hold: the host looks it up on the relay; the shell asks in the bar;
/// with Y it starts in a new tile with the focus, with N it is stopped.
#[test]
fn program_link_asks_then_starts() {
    let relay_dir = tempfile::tempdir().unwrap();
    let url = bobs_relay(relay_dir.path());
    let mut s = Sys::new(&format!("{}50 press y\n", follow(1)), Setup { relay: Some(url.clone()), ..Default::default() });
    s.run(45);
    let key = to_hex(&bob().feed_key());
    let short = format!("{}…{}", &key[..6], &key[58..]);
    assert!(s.bar().contains(&format!("reader asks to start Greeter by {short}.")), "{}", s.bar());
    assert_eq!(s.state(2), state::PAUSED, "loaded, waiting for the answer");
    assert_eq!(s.system.shell().machines().unwrap().slot(2).rect.w, 0, "not shown yet");
    assert_eq!(s.system.focus(), 0, "the keyboard is the shell's");
    golden("program-question", &s.dump());
    s.run(10);
    assert_eq!(s.state(2), state::RUNNING);
    assert_eq!(s.title(2), "greeter");
    assert_eq!(s.system.child(2).unwrap().identity, Some(ProgramId { publisher: bob().feed_key(), name: b"Greeter".to_vec() }));
    assert_eq!(s.system.focus(), 2);
    assert!(s.bar().contains("2 greeter"), "{}", s.bar());

    // N: stopped, the keyboard back to the reader.
    let mut s = Sys::new(&format!("{}50 press n\n", follow(1)), Setup { relay: Some(url), ..Default::default() });
    s.run(55);
    assert_eq!(s.state(2), state::NONE);
    assert_eq!(s.system.focus(), 1);
    assert!(s.bar().contains("1 reader") && !s.bar().contains("greeter"), "{}", s.bar());
}

/// A program the host cannot find anywhere: a brief message, no tile.
#[test]
fn program_link_not_found() {
    let mut s = Sys::new(&follow(1), Setup::default());
    s.run(48);
    assert_eq!(s.state(2), state::NONE);
    assert!(s.bar().contains("Could not start Greeter"), "{}", s.bar());
    s.run(200);
    assert!(s.bar().contains("1 reader"), "the message goes: {}", s.bar());
}

/// A link to an image by its ID: started paused by its ID (out of the
/// layout, so it runs no frame), asked about as an unpublished program
/// (its title and short ID), run with Y.
#[test]
fn image_link_asks_then_starts() {
    let mut s = Sys::new(&format!("{}50 press y\n", follow(2)), Setup::default());
    s.run(45);
    let id = to_hex(&blob_id(&greeter()));
    let short = format!("{}…{}", &id[..6], &id[58..]);
    assert!(s.bar().contains(&format!("reader asks to start an unpublished program greeter ({short}).")), "{}", s.bar());
    assert_eq!(s.state(2), state::PAUSED, "loaded, waiting for the answer");
    assert_eq!(s.system.shell().machines().unwrap().slot(2).rect.w, 0, "not shown yet");
    assert_eq!(s.machine(2).frame(), 0, "it has run no frame");
    golden("image-question", &s.dump());
    s.run(10);
    assert_eq!((s.state(2), s.title(2)), (state::RUNNING, "greeter".to_string()));
    assert_eq!(s.system.child(2).unwrap().identity, None);
}

/// A link to a sketch document: Sketch (built-in) starts at once with the
/// document as its argument and shows it.
#[test]
fn document_link_opens_sketch() {
    let mut s = Sys::new(&follow(3), Setup::default());
    s.run(60);
    assert_eq!((s.state(2), s.title(2)), (state::RUNNING, "sketch".to_string()));
    assert_eq!(s.system.child(2).unwrap().argument, Some(blob_id(&drawing())));
    assert_eq!(s.system.focus(), 2);
    let sketch = Sys::screen(s.machine(2));
    assert!(sketch.contains("Viewing a published sketch"), "{sketch}");
    assert!(s.machine(2).vector_list().len() > 40, "the strokes are drawn");
    golden("document-link", &s.dump());
}

/// A sheet document: the built-in sheet starts at once with the document
/// as its argument (the format's program, by the zero key and its name)
/// and shows it.
#[test]
fn sheet_document_opens_the_sheet() {
    if !common::have_tools() {
        return;
    }
    // The opener is the first built-in after the nine shipped ones.
    let script = "2 press down\n3 press down\n4 press down\n5 press down\n6 press down\n6 press down\n6 press down\n6 press down\n6 press down\n7 press enter\n20 press space\n";
    let mut s = Sys::new(script, Setup { extra: vec![opener(&blob_id(&sheet_document()))], ..Default::default() });
    s.run(80);
    assert_eq!(s.title(1), "opener");
    assert_eq!((s.state(2), s.title(2)), (state::RUNNING, "sheet".to_string()), "{}", s.dump());
    let child = s.system.child(2).unwrap();
    assert_eq!(child.argument, Some(blob_id(&sheet_document())));
    assert_eq!(child.identity, Some(ProgramId { publisher: [0; 32], name: b"sheet".to_vec() }));
    assert_eq!(s.system.focus(), 2);
    // The page's text as laid out (the sheet makes each character of the
    // content line an anchor, so the document's bytes have it in pieces).
    let p = s.machine(2).page().unwrap();
    let layout = p.document().map(|d| terse_core::page::reference_layout(d, p.region().w)).unwrap_or_default();
    let page: String = layout.lines.iter().map(|l| l.spans.iter().map(|s| s.text.as_str()).collect::<String>() + "\n").collect();
    assert!(page.contains("Lunch money") && page.contains("15"), "the document's cells, the total worked out: {page}");
}

/// The timeline's document entry (the second): Enter opens it in Sketch.
#[test]
fn timeline_document_opens_sketch() {
    let mut s = Sys::new("2 press enter\n20 press down\n22 press enter\n", Setup::default());
    s.run(19);
    golden("timeline", &s.dump());
    s.run(20);
    assert_eq!((s.state(2), s.title(2)), (state::RUNNING, "sketch".to_string()));
    assert_eq!(s.system.child(2).unwrap().argument, Some(blob_id(&drawing())));
}

/// "opener": a program that asks the shell to open blob `id` when a key
/// is pressed (built from C with programs/lib).
fn opener(id: &[u8; 32]) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("opener.c");
    let bytes: Vec<String> = id.iter().map(|b| b.to_string()).collect();
    std::fs::write(
        &src,
        format!(
            "#include \"terse.h\"\nstatic const uint8_t req[33] = {{1, {}}};\nint main(void)\n{{\n    terse_event e;\n    for (;;) {{\n        while (terse_event_pop(&e))\n            if (e.type == EV_KEY_DOWN)\n                terse_open(req);\n        terse_wait_frame();\n    }}\n}}\n",
            bytes.join(", ")
        ),
    )
    .unwrap();
    Image::build("opener", 1, &[terse_core::DeviceReq::new(2, 0)], &common::build_rom(&[src], true))
}

/// A request for an article (a blob that is neither an image nor a
/// document): a new reader tile opens it, given as its argument.
#[test]
fn article_opens_a_reader() {
    if !common::have_tools() {
        return;
    }
    let script = "2 press down\n3 press down\n4 press down\n5 press down\n6 press down\n6 press down\n6 press down\n6 press down\n6 press down\n7 press enter\n20 press space\n";
    let mut s = Sys::new(script, Setup { extra: vec![opener(&blob_id(SECOND))], ..Default::default() });
    s.run(50);
    assert_eq!(s.title(1), "opener");
    assert_eq!((s.state(2), s.title(2)), (state::RUNNING, "reader".to_string()), "{}", s.dump());
    assert_eq!(s.system.child(2).unwrap().argument, Some(blob_id(SECOND)));
    assert_eq!(s.system.focus(), 2);
    let page = s.machine(2).page().unwrap().document().map(|d| String::from_utf8_lossy(d).into_owned()).unwrap_or_default();
    assert!(page.contains("Opened in a tile of its own."), "{page}");
}

/// hello under the title "dicer", its image listing the experimental
/// device example.dice on slot 240 (spec/experimental.md).
fn dicer() -> Vec<u8> {
    let hello = Image::parse(builtin::PROGRAMS[3]).unwrap();
    let mut rom = hello.rom.clone();
    rom.extend(terse_core::image::experimental_table(&[(240, "example.dice".to_string())]));
    let mut devices = hello.devices.clone();
    devices.push(terse_core::DeviceReq::optional(240, 0));
    Image::build("dicer", hello.ram_pages, &devices, &rom)
}

/// A request for an image that lists an experimental device (SPEC
/// §machines.calls `experimental`): the shell starts it paused, and its
/// question has a second row saying that it can use one, which this host
/// does not give it; the tiles move down a row meanwhile. With Y it runs.
#[test]
fn experimental_device_in_the_question() {
    if !common::have_tools() {
        return;
    }
    let script = "2 press down\n3 press down\n4 press down\n5 press down\n6 press down\n6 press down\n6 press down\n6 press down\n6 press down\n7 press enter\n20 press space\n40 press y\n";
    let mut s = Sys::new(script, Setup { extra: vec![opener(&blob_id(&dicer())), dicer()], ..Default::default() });
    s.run(35);
    assert_eq!(s.title(1), "opener");
    assert_eq!(s.state(2), state::PAUSED, "loaded, waiting for the answer");
    let text = Sys::screen(s.system.shell());
    let rows: Vec<&str> = text.lines().collect();
    assert!(rows[0].contains("opener asks to start an unpublished program dicer (") && rows[0].ends_with("Y/N"), "{text}");
    assert_eq!(rows[1].trim(), "Can use an experimental device (example.dice); here it runs without it.", "{text}");
    assert_eq!(s.system.shell().machines().unwrap().slot(1).rect.y, 39, "a row lower");
    golden("experimental-question", &s.dump());
    s.run(10);
    assert_eq!((s.state(2), s.title(2)), (state::RUNNING, "dicer".to_string()));
    assert!(s.bar().contains("2 ⚗dicer"), "{}", s.bar());
    assert_eq!(s.system.shell().machines().unwrap().slot(1).rect.y, 23, "back in place");
}

/// Eight machines: a request finds no free handle; the bar says so.
#[test]
fn no_free_handle() {
    // Seven hellos from the launcher, then the reader (handle 8).
    let mut script = String::from("2 press down\n3 press down\n4 press down\n5 press enter\n");
    let mut f = 10;
    for i in 0..7u32 {
        script += &format!("{f} home\n");
        f += 1;
        let ups = if i < 6 { 6 + i } else { 9 + i };
        for _ in 0..ups {
            script += &format!("{f} press up\n");
            f += 1;
        }
        script += &format!("{f} press enter\n");
        f += 10;
    }
    let base = f + 10;
    script += &format!("{} press enter\n{} press tab\n{} press tab\n{} press tab\n{} press enter\n", base, base + 10, base + 11, base + 12, base + 20);
    let mut s = Sys::new(&script, Setup::default());
    s.run(base + 25);
    assert!((1..=8).all(|h| s.state(h) == state::RUNNING));
    assert_eq!(s.title(8), "reader");
    assert!(s.bar().contains("No free handle"), "{}", s.bar());
}

/// The same machine asks again at once: one request a second is taken;
/// after a no, its requests are dropped for ten seconds.
#[test]
fn requests_are_limited() {
    let script = format!("{}42 press enter\n50 press n\n55 press enter\n", follow(2));
    let mut s = Sys::new(&script, Setup::default());
    s.run(45);
    assert!(s.bar().contains("unpublished program"), "{}", s.bar());
    s.run(20);
    assert_eq!(s.state(2), state::NONE);
    assert!(!s.bar().contains("Y/N"), "the request after the no is dropped: {}", s.bar());
}

/// Sketch: draw, save, reload; publish a document to a relay from one
/// host; another host following the author gets the entry and opens it
/// from its timeline in Sketch, which shows the same strokes.
#[test]
fn sketch_round_trip_between_hosts() {
    let relay_dir = tempfile::tempdir().unwrap();
    let relay = Arc::new(terse_relay::Relay::new(BlobStore::open(relay_dir.path(), None).unwrap(), 1024));
    let url = serve(relay.clone());
    let (a_store, a_data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let dirs = Some((a_store.path().to_path_buf(), a_data.path().to_path_buf()));
    // Sketch alone fills the area: its picture is at (17, 23), 605 x 453.
    let at = |x: u32, y: u32| (17 + x * 605 / 640, 23 + y * 453 / 480 + 1);
    let mut script = String::from("2 press down\n3 press down\n4 press down\n5 press down\n6 press enter\n");
    let points = [(100, 100), (200, 150), (300, 120), (400, 260)];
    for (i, &(x, y)) in points.iter().enumerate() {
        let (sx, sy) = at(x, y);
        script += &format!("{} pointer {sx} {sy} 1\n", 20 + i);
    }
    let (sx, sy) = at(400, 260);
    script += &format!("24 pointer {sx} {sy} 0\n30 press s\n40 press p\n");
    let mut a = Sys::new(&script, Setup { relay: Some(url.clone()), consent: Consent::Allow, dirs: dirs.clone(), content: false, extra: Vec::new() });
    a.run(60);
    assert_eq!(a.title(1), "sketch");
    let drawn = a.machine(1).vector_list().to_vec();
    assert!(Sys::screen(a.machine(1)).contains("Published as Sketch 1"), "{}", Sys::screen(a.machine(1)));
    golden("sketch-drawn", &a.dump());
    // The record was saved: a new run of Sketch on host A loads it.
    drop(a);
    let mut again = Sys::new("2 press down\n3 press down\n4 press down\n5 press down\n6 press enter\n", Setup { dirs, content: false, ..Default::default() });
    again.run(30);
    assert_eq!(again.machine(1).vector_list(), &drawn[..], "reloaded from the record");
    // The entry reached the relay as a document.
    let held = relay.feeds().entries(&alice().feed_key()).unwrap();
    assert_eq!((held.len(), held[0].entry.kind, held[0].entry.title.as_str()), (1, 4, "Sketch 1"));
    // Host B follows Alice through the relay; its timeline has the
    // document; Enter opens it in Sketch.
    let (b_store, b_data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut node = Node::new(BlobStore::open(b_store.path(), None).unwrap(), None);
    node.add_relay(&url, RelayClient::websocket(&url, DEFAULT_TIMEOUT));
    node.set_subscriptions(Subscriptions::in_memory([alice().feed_key()]));
    node.set_records(terse_net::Records::in_data_dir(b_data.path()));
    let ids = builtin::install(node.store(), &[]).unwrap();
    let mut content = ContentService::start_with_consent(node, Consent::Deny);
    content.wait_caught_up(Duration::from_secs(10)).unwrap();
    let shell = Image::parse(builtin::SHELL).unwrap();
    let mut system = System::new(&shell, true, ids);
    system.set_shell_identity(Some(ProgramId { publisher: [0; 32], name: shell.title.clone() }));
    let headless = HeadlessSystem::new(Some(content), Script::parse("2 press enter\n20 press enter\n").unwrap());
    let mut b = Sys { system, headless, _dirs: Vec::new(), shell_units: Vec::new(), twin: None };
    b.run(40);
    assert_eq!((b.state(2), b.title(2)), (state::RUNNING, "sketch".to_string()));
    assert_eq!(b.system.child(2).unwrap().argument, Some(held[0].entry.blob));
    assert!(Sys::screen(b.machine(2)).contains("Viewing a published sketch"), "{}", Sys::screen(b.machine(2)));
    // The same strokes (the list less the status tail: compare the drawn
    // part by length and its first bytes).
    let shown = b.machine(2).vector_list();
    assert_eq!(shown.len(), drawn.len());
    assert_eq!(&shown[..shown.len() - 32], &drawn[..drawn.len() - 32]);
}

/// The shell's frames while it handles requests.
#[test]
fn budget() {
    let mut s = Sys::new(&format!("{}50 press y\n", follow(2)), Setup::default());
    s.run(80);
    let stats = common::FrameStats::new(&s.shell_units);
    println!("shell: {}", stats.describe());
    assert!(stats.busiest < terse_core::BUDGET_PER_FRAME as u64 / 8, "{}", stats.describe());
}
