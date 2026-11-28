//! The page's host run natively: the shell starts, its launcher opens the
//! manual in the reader, a program runs alone, and the draw list holds what
//! the screens show.

use std::collections::HashMap;

use terse_web::bundle::{self, BLOB, MANUAL_INDEX, PROGRAM, SHELL};
use terse_web::draw::{self, Ops};
use terse_web::{Host, Start};

const PROGRAMS: [&str; 9] = ["reader", "sparks", "fetch", "hello", "sketch", "holo", "reports", "reef", "sheet"];

/// The bundle `terse manual site` writes, from the repository's files.
fn bundle() -> Vec<u8> {
    let root = terse_testkit::workspace();
    let read = |p: &str| std::fs::read(root.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let mut entries: Vec<(u8, Vec<u8>)> = vec![(SHELL, read("programs/shell/shell.terse"))];
    for p in PROGRAMS.iter().chain(["lowbell"].iter()) {
        entries.push((PROGRAM, read(&format!("programs/{p}/{p}.terse"))));
    }
    entries.push((BLOB, read("programs/fetch/message.txt")));
    let manifest = String::from_utf8(read("docs/manual/build/manifest")).unwrap();
    for line in manifest.lines().filter(|l| !l.is_empty()) {
        let name = line.split(' ').nth(1).unwrap();
        entries.push((if name == "index" { MANUAL_INDEX } else { BLOB }, read(&format!("docs/manual/build/{name}.txt"))));
    }
    let refs: Vec<(u8, &[u8])> = entries.iter().map(|(k, d)| (*k, d.as_slice())).collect();
    bundle::encode(&refs)
}

struct Run {
    host: Host,
    last: HashMap<u32, Vec<u8>>,
    ops: Ops,
}

impl Run {
    fn new(start: Start) -> Run {
        Run { host: Host::new(&bundle(), start).unwrap(), last: HashMap::new(), ops: Ops::default() }
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            assert!(self.host.frame(0), "the system stopped");
        }
    }

    fn press(&mut self, usage: u8) {
        self.host.input.key(true, usage, 0);
        self.frames(1);
        self.host.input.key(false, usage, 0);
        self.frames(3);
    }

    /// All the text the screens draw now (every screen, as if new).
    fn text(&mut self) -> String {
        self.last.clear();
        draw::frame(&self.host, &mut self.last, &mut self.ops);
        draw::texts(&self.ops.buf).join("\n")
    }
}

#[test]
fn shell_opens_the_manual() {
    let mut r = Run::new(Start::Shell);
    r.frames(30);
    let text = r.text();
    assert!(text.contains("Manual"), "the launcher lists the manual:\n{text}");
    assert!(text.contains("sparks") && text.contains("lowbell"), "and the programs:\n{text}");
    // The cursor starts on the first program; Up, then Enter on "Manual"
    // opens the reader on the index.
    r.press(0x52);
    r.press(0x28);
    r.frames(60);
    assert!(r.host.system.children().any(|(_, c)| c.title == "reader" && c.machine.is_some()), "a reader runs");
    let text = r.text();
    assert!(text.contains("Tersenet") && text.contains("way to share information"), "the reader shows the index:\n{text}");
    // A screen that did not change is given once.
    r.frames(1);
    draw::frame(&r.host, &mut r.last, &mut r.ops);
    let again = r.ops.buf.len();
    draw::frame(&r.host, &mut r.last, &mut r.ops);
    assert!(r.ops.buf.len() <= again);
}

#[test]
fn programs_run_alone() {
    for title in ["sparks", "sheet", "sketch", "lowbell", "reader"] {
        let mut r = Run::new(Start::Alone(title));
        r.frames(120);
        assert!(r.host.system.stopped().is_none(), "{title} runs");
        let mut last = HashMap::new();
        let mut ops = Ops::default();
        draw::frame(&r.host, &mut last, &mut ops);
        assert!(ops.buf.len() > 20, "{title} draws something");
    }
    let mut r = Run::new(Start::Alone("reader"));
    r.frames(60);
    let text = r.text();
    assert!(text.contains("way to share information"), "the reader alone opens the manual:\n{text}");
    assert!(Host::new(&bundle(), Start::Alone("nothing")).is_err());
}

/// The machines keep to their budget and run the same given the same input
/// (the clock at 0, random bytes 0 outside the page).
#[test]
fn deterministic() {
    let hashes = || {
        let mut r = Run::new(Start::Shell);
        r.frames(20);
        r.press(0x28);
        r.frames(40);
        let mut h: Vec<[u8; 32]> = vec![r.host.system.shell().state_hash()];
        h.extend(r.host.system.children().filter_map(|(_, c)| c.machine.as_ref().map(|m| m.state_hash())));
        assert!(r.host.used.iter().all(|u| u.1 <= 2 * 125_000), "{:?}", r.host.used);
        h
    };
    assert_eq!(hashes(), hashes());
}
