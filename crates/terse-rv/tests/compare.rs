//! Profile 1 (Z80) against profile 2 (RV32IMC) for hello, the reader and
//! sparks: sizes, stack depth, budget use, host cost.
//!
//! `harness_is_exact` (always run) checks that the instruction-counting Z80
//! machine in z80harness/ gives exactly `terse_core::Machine`'s results.
//! The measurements are ignored tests that print; run them in release:
//!
//!     cargo test --release -p terse-rv --test compare -- --ignored --nocapture --test-threads 1

mod common;
#[path = "z80harness/mod.rs"]
mod z80harness;

use std::time::Instant;

use terse_core::{Event, FrameInput, FrameMachine, FrameResult, Image, Machine, Script, TSTATES_PER_FRAME};
use terse_net::service::Headless;
use terse_net::{BlobStore, ContentService, Draft, FeedKey, Identity, Node, Subscriptions, blob_id};
use terse_rv::{INSTRUCTIONS_PER_FRAME, RAM_BASE, RvMachine};
use z80harness::Z80Counter;

const Z80_HELLO: &[u8] = include_bytes!("../../../programs/hello/hello.terse");
const Z80_READER: &[u8] = include_bytes!("../../../programs/reader/reader.terse");
const Z80_SPARKS: &[u8] = include_bytes!("../../../programs/sparks/sparks.terse");
const RV_HELLO: &[u8] = include_bytes!("../../../programs/rv/hello/hello.terse");
const RV_READER: &[u8] = include_bytes!("../../../programs/rv/reader/reader.terse");
const RV_SPARKS: &[u8] = include_bytes!("../../../programs/rv/sparks/sparks.terse");

fn z80_image(file: &[u8]) -> Image {
    Image::parse(file).unwrap()
}

fn rv(file: &[u8]) -> RvMachine {
    RvMachine::new(&terse_rv::parse_image(file).unwrap())
}

// ---- The reader's fixture (as in crates/terse-cli/tests/reader.rs) ----

const T0: u32 = 1_794_938_400;

fn alice() -> Identity {
    Identity::from_seed([11; 32])
}

fn bob() -> Identity {
    Identity::from_seed([12; 32])
}

fn long_article() -> String {
    let mut text = String::from("A long article\n\n");
    for p in 1..=20 {
        text.push_str(&format!(
            "Paragraph {p}. The quick brown fox jumps over the lazy dog while the reader wraps each line at a \
             space so that no word is split unless it is longer than the row itself. 日本語の文章も二桁の幅で\
             折り返されます。\tA tab, then more words to fill the paragraph out to a few hundred bytes.\n\n"
        ));
    }
    text.push_str(&"x".repeat(100));
    text.push_str("\nThe end.\n");
    text
}

fn big_article() -> String {
    (1..=400).map(|i| format!("Line {i:03} of the big article, which is longer than one 16 KiB request.\n")).collect()
}

struct Fixture {
    store: tempfile::TempDir,
    author: Node,
    feeds: Vec<FeedKey>,
    time: u32,
}

impl Fixture {
    fn new() -> Self {
        let store = tempfile::tempdir().unwrap();
        let author = Node::new(BlobStore::open(store.path(), None).unwrap(), None);
        Fixture { store, author, feeds: vec![alice().feed_key(), bob().feed_key()], time: T0 }
    }

    fn publish(&mut self, who: &Identity, title: &str, data: &[u8], reply_to: Option<[u8; 32]>) {
        self.author.set_identity(who.clone());
        self.time += 600;
        let draft = Draft { data: data.to_vec(), title: title.into(), reply_to, time: self.time };
        assert!(matches!(self.author.publish_article(&draft), terse_net::Done::Published { .. }));
    }

    fn populated() -> Self {
        let mut f = Fixture::new();
        let welcome = b"Welcome to the Terse reader.\n\nThis article is short.\n".to_vec();
        f.publish(&alice(), "Welcome to the Terse reader", &welcome, None);
        f.publish(&bob(), "日本語の記事：テスト", "日本語の本文です。\n二行目。\n".as_bytes(), None);
        f.publish(&alice(), "A long article", long_article().as_bytes(), None);
        f.publish(&bob(), "Re: Welcome to the Terse reader", b"Thanks, this is a reply.\n", Some(blob_id(&welcome)));
        f.publish(&alice(), "An article that is not in the store", b"gone", None);
        std::fs::remove_file(f.store.path().join(terse_net::to_hex(&blob_id(b"gone")))).unwrap();
        f.publish(
            &bob(),
            "A title long enough that the list has to cut it short with an ellipsis, wide characters: 長い題名",
            b"Short.\n",
            None,
        );
        f.publish(&alice(), "A big article (over 16 KiB)", big_article().as_bytes(), None);
        f.publish(&bob(), "Emoji 😀 and tabs\tin a title", b"Text.\n", None);
        f
    }

    /// One entry: an all-wide article of about 16 KiB.
    fn wide() -> Self {
        let mut f = Fixture::new();
        let text: String = (0..80).map(|i| format!("{}\n", "日本語の文章も二桁の幅で折り返されます。".repeat(1 + i % 4))).collect();
        f.publish(&alice(), "日本語".repeat(13).as_str(), text.as_bytes(), None);
        f
    }

    fn content(&self) -> ContentService {
        let mut node = Node::new(BlobStore::open(self.store.path(), None).unwrap(), None);
        node.set_subscriptions(Subscriptions::in_memory(self.feeds.iter().copied()));
        ContentService::start(node)
    }
}

/// The session of `timing` in crates/terse-cli/tests/reader.rs: open the
/// ~6 KB article at frame 50 and the >16 KiB one at frame 123.
const READER_SCRIPT: &str = "40 press down\n41 press down\n42 press down\n43 press down\n44 press down\n50 press enter\n\
     90 press pagedown\n95 wheel 2\n100 press end\n110 press esc\n120 press home\n121 press down\n\
     123 press enter\n160 press end\n170 press esc\n";
const WIDE_SCRIPT: &str = "14 press enter\n40 press pagedown\n41 press pagedown\n42 press end\n50 press home\n51 wheel 5\n";

/// Runs the reader as `terse run` does; returns each frame's result and screen.
fn reader_session(m: &mut impl FrameMachine, fixture: &Fixture, script: &str, frames: u32) -> (Vec<FrameResult>, Vec<String>) {
    let mut content = fixture.content();
    let script = Script::parse(script).unwrap();
    let mut completions = Vec::new();
    let (mut results, mut screens) = (Vec::new(), Vec::new());
    for _ in 0..frames {
        let events = script.events(m.frame()).to_vec();
        let done = std::mem::take(&mut completions);
        let r = m.run_frame(&FrameInput { events: &events, completions: &done, ..Default::default() });
        content.submit(m.take_content_requests());
        completions = content.wait_finished();
        results.push(r);
        screens.push(m.terminal().unwrap().text());
    }
    (results, screens)
}

/// The first frame from which rows 0-28 (or all rows with `status`) stay
/// as they are at frame `to`.
fn settled(screens: &[String], from: usize, to: usize, status: bool) -> usize {
    let rows = |s: &str| s.lines().take(if status { 30 } else { 29 }).collect::<Vec<_>>().join("\n");
    let last = rows(&screens[to]);
    (from..=to).find(|&i| screens[i..=to].iter().all(|s| rows(s) == last)).unwrap()
}

fn sparks_session(m: &mut impl FrameMachine, name: &str, frames: u32) -> Vec<FrameResult> {
    let script = std::fs::read_to_string(common::workspace().join("programs/sparks/sessions").join(name)).unwrap();
    let mut h = Headless::new(None, Script::parse(&script).unwrap());
    (0..frames).map(|_| h.frame(m)).collect()
}

fn hello_events(frame: u32) -> Vec<Event> {
    let text = |s: &str| s.chars().map(Event::text).collect::<Vec<_>>();
    match frame {
        5 => text("Hi wörld "),
        6 => text("你好"),
        7 => vec![Event::key_down(0x28, 0), Event::key_up(0x28, 0)],
        8 => text("😀x"),
        9 => vec![Event::key_down(0x2A, 0)],
        10 => text("ok"),
        _ => vec![],
    }
}

fn hello_session(m: &mut impl FrameMachine, frames: u32) -> Vec<FrameResult> {
    (0..frames)
        .map(|f| {
            let events = hello_events(f);
            m.run_frame(&FrameInput { events: &events, unix_time: 1_700_000_000 + f / 60, ..Default::default() })
        })
        .collect()
}

#[test]
fn harness_is_exact() {
    let (mut real, mut counted) = (Machine::new(&z80_image(Z80_SPARKS)), Z80Counter::new(&z80_image(Z80_SPARKS)));
    let a = sparks_session(&mut real, "keys.txt", 1200);
    let b = sparks_session(&mut counted, "keys.txt", 1200);
    assert_eq!(a, b);
    assert_eq!(real.vector_list(), counted.vector_list());
    assert_eq!(real.terminal(), counted.terminal());
    let f = Fixture::populated();
    let (mut real, mut counted) = (Machine::new(&z80_image(Z80_READER)), Z80Counter::new(&z80_image(Z80_READER)));
    let a = reader_session(&mut real, &f, READER_SCRIPT, 200);
    let b = reader_session(&mut counted, &f, READER_SCRIPT, 200);
    assert_eq!(a, b);
    assert!(counted.instructions > 0);
    let (mut real, mut counted) = (Machine::new(&z80_image(Z80_HELLO)), Z80Counter::new(&z80_image(Z80_HELLO)));
    assert_eq!(hello_session(&mut real, 30), hello_session(&mut counted, 30));
}

fn pct(v: u64, of: i64) -> String {
    format!("{:.2}%", v as f64 * 100.0 / of as f64)
}

/// T-states and instructions of a Z80 frame, as one cell.
fn z80_cell(t: u64, i: u64) -> String {
    format!("{t} T ({}) = {i} instr ({:.1} T/instr)", pct(t, TSTATES_PER_FRAME), t as f64 / i.max(1) as f64)
}

fn rv_cell(i: u64) -> String {
    format!("{i} instr ({})", pct(i, INSTRUCTIONS_PER_FRAME))
}

fn median(v: &[u64]) -> u64 {
    let mut s = v.to_vec();
    s.sort_unstable();
    s[s.len() / 2]
}

/// Sizes, stack depth and budget use. Prints a table.
#[test]
#[ignore]
fn report_budget() {
    let ram_end = |pages: u8| RAM_BASE + pages as u32 * 8192;

    // ---- hello ----
    let mut z = Z80Counter::new(&z80_image(Z80_HELLO));
    let mut zi = Vec::new();
    let zr: Vec<FrameResult> = (0..30)
        .map(|f| {
            let events = hello_events(f);
            let r = z.run_frame(&FrameInput { events: &events, unix_time: 1_700_000_000 + f / 60, ..Default::default() });
            zi.push(z.frame_instructions);
            r
        })
        .collect();
    let mut r = rv(RV_HELLO);
    let mut ri = Vec::new();
    let rr: Vec<FrameResult> = (0..30)
        .map(|f| {
            let before = r.instructions();
            let events = hello_events(f);
            let res = r.run_frame(&FrameInput { events: &events, unix_time: 1_700_000_000 + f / 60, ..Default::default() });
            ri.push(r.instructions() - before);
            res
        })
        .collect();
    assert_eq!(z.terminal().unwrap().text(), r.terminal().unwrap().text(), "hello: same screen");
    let zt: Vec<u64> = zr.iter().map(|x| x.tstates).collect();
    let rt: Vec<u64> = rr.iter().map(|x| x.tstates).collect();
    let zb = (0..30).max_by_key(|&i| zt[i]).unwrap();
    let rb = (0..30).max_by_key(|&i| rt[i]).unwrap();
    println!("== hello (30 frames, the golden-test input) ==");
    println!("ROM: Z80 {} bytes, RV {} bytes", z80_image(Z80_HELLO).rom.len(), terse_rv::parse_image(RV_HELLO).unwrap().rom.len());
    println!("stack: Z80 {} bytes, RV {} bytes", 0x1_0000 - z.min_sp, ram_end(4) - r.cpu().min_sp);
    println!("first full draw (frame 0): Z80 {} | RV {}", z80_cell(zt[0], zi[0]), rv_cell(rt[0]));
    println!("busiest frame: Z80 #{zb} {} | RV #{rb} {}", z80_cell(zt[zb], zi[zb]), rv_cell(rt[rb]));
    println!("median frame: Z80 {} T | RV {} instr", median(&zt), median(&rt));

    // ---- sparks ----
    println!("\n== sparks (recorded sessions, 3,600 frames each) ==");
    println!(
        "ROM: Z80 {} bytes, RV {} bytes",
        z80_image(Z80_SPARKS).rom.len(),
        terse_rv::parse_image(RV_SPARKS).unwrap().rom.len()
    );
    for name in ["keys.txt", "pad.txt"] {
        let mut z = Z80Counter::new(&z80_image(Z80_SPARKS));
        let script = std::fs::read_to_string(common::workspace().join("programs/sparks/sessions").join(name)).unwrap();
        let mut h = Headless::new(None, Script::parse(&script).unwrap());
        let (mut zt, mut zi) = (Vec::new(), Vec::new());
        for _ in 0..3600 {
            zt.push(h.frame(&mut z).tstates);
            zi.push(z.frame_instructions);
        }
        let mut r = rv(RV_SPARKS);
        let rt: Vec<u64> = sparks_session(&mut r, name, 3600).iter().map(|x| x.tstates).collect();
        assert_eq!(z.vector_list(), r.vector_list(), "{name}: same display list at the end");
        let zb = (1..3600).max_by_key(|&i| zt[i]).unwrap();
        let rb = (1..3600).max_by_key(|&i| rt[i]).unwrap();
        let zd = (1..60).max_by_key(|&i| zt[i]).unwrap();
        let rd = (1..60).max_by_key(|&i| rt[i]).unwrap();
        println!("-- {name} --");
        println!("stack: Z80 {} bytes, RV {} bytes", 0x1_0000 - z.min_sp, ram_end(4) - r.cpu().min_sp);
        println!("first full draw (frame 0, setting up): Z80 {} | RV {}", z80_cell(zt[0], zi[0]), rv_cell(rt[0]));
        println!("busiest frame after 0: Z80 #{zb} {} | RV #{rb} {}", z80_cell(zt[zb], zi[zb]), rv_cell(rt[rb]));
        println!("demo, 6 sweepers + 8 sparks, worst: Z80 #{zd} {} | RV #{rd} {}", z80_cell(zt[zd], zi[zd]), rv_cell(rt[rd]));
        let zm = median(&zt[1..]);
        let zmi = median(&zi[1..]);
        println!("median frame: Z80 {} | RV {}", z80_cell(zm, zmi), rv_cell(median(&rt[1..])));
        println!(
            "total: Z80 {} instr, {} T; RV {} instr ({} calls)",
            z.instructions,
            zt.iter().sum::<u64>(),
            r.instructions(),
            r.calls()
        );
    }

    // ---- reader ----
    println!("\n== reader (the `timing` session of crates/terse-cli/tests/reader.rs, 200 frames) ==");
    println!(
        "ROM: Z80 {} bytes, RV {} bytes",
        z80_image(Z80_READER).rom.len(),
        terse_rv::parse_image(RV_READER).unwrap().rom.len()
    );
    let f = Fixture::populated();
    let mut z = Z80Counter::new(&z80_image(Z80_READER));
    let mut zi = Vec::new();
    let (zr, zs) = {
        // Per-frame instruction counts need the counter after each frame.
        let mut content = f.content();
        let script = Script::parse(READER_SCRIPT).unwrap();
        let mut completions = Vec::new();
        let (mut results, mut screens) = (Vec::new(), Vec::new());
        for _ in 0..200 {
            let events = script.events(z.frame()).to_vec();
            let done = std::mem::take(&mut completions);
            results.push(z.run_frame(&FrameInput { events: &events, completions: &done, ..Default::default() }));
            zi.push(z.frame_instructions);
            content.submit(z.take_content_requests());
            completions = content.wait_finished();
            screens.push(z.terminal().unwrap().text());
        }
        (results, screens)
    };
    let mut r = rv(RV_READER);
    let (rr, rs) = reader_session(&mut r, &f, READER_SCRIPT, 200);
    let zt: Vec<u64> = zr.iter().map(|x| x.tstates).collect();
    let rt: Vec<u64> = rr.iter().map(|x| x.tstates).collect();
    let (zl, rl) = (settled(&zs, 0, 39, true), settled(&rs, 0, 39, true));
    let sum = |v: &[u64], to: usize| v[..=to].iter().sum::<u64>();
    println!("stack: Z80 {} bytes, RV {} bytes", 0x1_0000 - z.min_sp, ram_end(10) - r.cpu().min_sp);
    println!(
        "first full draw (list of 8 entries): Z80 by frame {zl}, {} T = {} instr | RV by frame {rl}, {} instr",
        sum(&zt, zl),
        sum(&zi, zl),
        sum(&rt, rl)
    );
    for (what, at, to) in [("~6 KB article", 50, 89), (">16 KiB article", 123, 159)] {
        println!(
            "Enter to {what}: first page Z80 {} frames, RV {} frames; fully wrapped Z80 {} frames, RV {} frames; \
             work Z80 {} T = {} instr, RV {} instr",
            settled(&zs, at, to, false) - at + 1,
            settled(&rs, at, to, false) - at + 1,
            settled(&zs, at, to, true) - at + 1,
            settled(&rs, at, to, true) - at + 1,
            zt[at..=settled(&zs, at, to, true)].iter().sum::<u64>(),
            zi[at..=settled(&zs, at, to, true)].iter().sum::<u64>(),
            rt[at..=settled(&rs, at, to, true)].iter().sum::<u64>(),
        );
    }
    let zb = (0..200).max_by_key(|&i| zt[i]).unwrap();
    let rb = (0..200).max_by_key(|&i| rt[i]).unwrap();
    println!("busiest frame: Z80 #{zb} {} | RV #{rb} {}", z80_cell(zt[zb], zi[zb]), rv_cell(rt[rb]));
    println!("median frame: Z80 {} | RV {}", z80_cell(median(&zt), median(&zi)), rv_cell(median(&rt)));
    println!("total: Z80 {} instr, {} T; RV {} instr ({} calls)", z.instructions, zt.iter().sum::<u64>(), r.instructions(), r.calls());

    let f = Fixture::wide();
    let mut z = Z80Counter::new(&z80_image(Z80_READER));
    let (zr, _) = reader_session(&mut z, &f, WIDE_SCRIPT, 70);
    let mut r = rv(RV_READER);
    let (rr, _) = reader_session(&mut r, &f, WIDE_SCRIPT, 70);
    let zw = zr.iter().map(|x| x.tstates).max().unwrap();
    let rw = rr.iter().map(|x| x.tstates).max().unwrap();
    let r_over = rr.iter().filter(|x| !x.halted_early).count();
    println!(
        "wide text (16 KiB all-wide article, 70 frames): busiest Z80 {zw} T ({}), RV {rw} instr ({}), \
         RV frames that ran out of budget: {r_over}; total Z80 {} instr, RV {} instr",
        pct(zw, TSTATES_PER_FRAME),
        pct(rw, INSTRUCTIONS_PER_FRAME),
        z.instructions,
        r.instructions()
    );
}

// ---- Host cost ----

/// Z80 spin: copy and add over 256 bytes in a loop (ld a,(hl); add a,b;
/// ld (de),a; inc hl; inc de; djnz), restarting forever.
const Z80_SPIN: [u8; 18] = [
    0x21, 0x00, 0x80, // ld hl, 8000h
    0x11, 0x00, 0x90, // ld de, 9000h
    0x06, 0x00, //       ld b, 0
    0x7E, //             loop: ld a, (hl)
    0x80, //             add a, b
    0x12, //             ld (de), a
    0x23, //             inc hl
    0x13, //             inc de
    0x10, 0xF9, //       djnz loop
    0xC3, 0x00, 0x00, // jp 0
];

const RV_SPIN: &str = "    li s0, 0x100000\n    li s1, 0x101000\n1:  li t2, 256\n    mv t0, s0\n    mv t1, s1\n\
     2:  lbu a0, 0(t0)\n    add a0, a0, t2\n    sb a0, 0(t1)\n    addi t0, t0, 1\n    addi t1, t1, 1\n\
     addi t2, t2, -1\n    bnez t2, 2b\n    j 1b\n";

/// Microseconds per frame (the best of `reps` runs of `frames` frames).
fn time_frames(mut make: impl FnMut() -> Box<dyn FrameMachine>, frames: u32, reps: u32) -> f64 {
    (0..reps)
        .map(|_| {
            let mut m = make();
            let t = Instant::now();
            for _ in 0..frames {
                m.run_frame(&FrameInput::default());
            }
            t.elapsed().as_secs_f64() * 1e6 / frames as f64
        })
        .fold(f64::INFINITY, f64::min)
}

/// Wall-clock time of the frames of a session, each timed alone, best of `reps`.
fn time_session_frames(mut run: impl FnMut(&mut Vec<f64>), reps: u32) -> Vec<f64> {
    let mut best: Vec<f64> = Vec::new();
    for _ in 0..reps {
        let mut t = Vec::new();
        run(&mut t);
        if best.is_empty() {
            best = t;
        } else {
            for (b, v) in best.iter_mut().zip(t) {
                *b = b.min(v);
            }
        }
    }
    best
}

#[test]
#[ignore]
fn report_host_cost() {
    if cfg!(debug_assertions) {
        eprintln!("run in release: cargo test --release -p terse-rv --test compare -- --ignored --nocapture");
    }
    // Full-budget frames.
    let z_image = z80_image(&Image::build("spin", 4, &[], &Z80_SPIN));
    let mut zc = Z80Counter::new(&z_image);
    zc.run_frame(&FrameInput::default());
    let z_ipf = zc.frame_instructions;
    let z_us = time_frames(|| Box::new(Machine::new(&z_image)), 600, 5);
    println!(
        "Z80 spin: {TSTATES_PER_FRAME} T-states = {z_ipf} instructions a frame; {z_us:.0} us a frame; \
         {:.1} M instructions/s, {:.1} M T-states/s",
        z_ipf as f64 / z_us,
        TSTATES_PER_FRAME as f64 / z_us
    );
    if common::have_tools() {
        let rom = common::assemble(RV_SPIN);
        let program = common::program(&rom, 4, &[]);
        let r_us = time_frames(|| Box::new(RvMachine::new(&program)), 600, 5);
        println!(
            "RV spin: {INSTRUCTIONS_PER_FRAME} instructions a frame; {r_us:.0} us a frame; {:.1} M instructions/s",
            INSTRUCTIONS_PER_FRAME as f64 / r_us
        );
    }

    // The busiest real frames.
    let busiest = |name: &str, costs: &[u64], times: &[f64], unit: &str| {
        let i = (0..costs.len()).max_by_key(|&i| costs[i]).unwrap();
        let total: f64 = times.iter().sum();
        println!(
            "{name}: busiest frame #{i} ({} {unit}) took {:.0} us; all {} frames {:.1} ms ({:.1} us a frame)",
            costs[i],
            times[i],
            times.len(),
            total / 1000.0,
            total * 1000.0 / times.len() as f64
        );
    };
    for (label, file, rvfile) in [("sparks keys.txt", Z80_SPARKS, RV_SPARKS)] {
        let script = std::fs::read_to_string(common::workspace().join("programs/sparks/sessions/keys.txt")).unwrap();
        for profile in [1, 2] {
            let mut costs = Vec::new();
            let times = time_session_frames(
                |t| {
                    let mut m: Box<dyn FrameMachine> = if profile == 1 {
                        Box::new(Machine::new(&z80_image(file)))
                    } else {
                        Box::new(rv(rvfile))
                    };
                    let mut h = Headless::new(None, Script::parse(&script).unwrap());
                    costs.clear();
                    for _ in 0..3600 {
                        let s = Instant::now();
                        let r = h.frame(&mut m);
                        t.push(s.elapsed().as_secs_f64() * 1e6);
                        costs.push(r.tstates);
                    }
                },
                5,
            );
            busiest(&format!("{label} profile {profile}"), &costs, &times, if profile == 1 { "T" } else { "instr" });
        }
    }
    for (label, fixture, script, frames) in
        [("reader timing", Fixture::populated(), READER_SCRIPT, 200), ("reader wide", Fixture::wide(), WIDE_SCRIPT, 70)]
    {
        for profile in [1, 2] {
            let mut costs = Vec::new();
            let times = time_session_frames(
                |t| {
                    let mut m: Box<dyn FrameMachine> = if profile == 1 {
                        Box::new(Machine::new(&z80_image(Z80_READER)))
                    } else {
                        Box::new(rv(RV_READER))
                    };
                    let mut content = fixture.content();
                    let script = Script::parse(script).unwrap();
                    let mut completions = Vec::new();
                    costs.clear();
                    for _ in 0..frames {
                        let events = script.events(m.frame()).to_vec();
                        let done = std::mem::take(&mut completions);
                        let s = Instant::now();
                        let r = m.run_frame(&FrameInput { events: &events, completions: &done, ..Default::default() });
                        t.push(s.elapsed().as_secs_f64() * 1e6);
                        costs.push(r.tstates);
                        content.submit(m.take_content_requests());
                        completions = content.wait_finished();
                    }
                },
                5,
            );
            busiest(&format!("{label} profile {profile}"), &costs, &times, if profile == 1 { "T" } else { "instr" });
        }
    }
}

/// Where the RV reader's instructions go on the wide-text article: writes
/// "pc count" lines to $PROFILE_OUT (map them to functions with llvm-nm on
/// an ELF built like programs/rv/reader/build.sh).
#[test]
#[ignore]
fn profile_reader_wide() {
    let Some(out) = std::env::var_os("PROFILE_OUT") else { return };
    let mut r = rv(RV_READER);
    r.set_profiling(true);
    let fixture = if std::env::var_os("PROFILE_TIMING").is_some() { Fixture::populated() } else { Fixture::wide() };
    let script = if std::env::var_os("PROFILE_TIMING").is_some() { READER_SCRIPT } else { WIDE_SCRIPT };
    reader_session(&mut r, &fixture, script, 200);
    let mut lines: Vec<(u32, u64)> = r.profile().unwrap().iter().map(|(&pc, &n)| (pc, n)).collect();
    lines.sort_unstable();
    std::fs::write(out, lines.iter().map(|(pc, n)| format!("{pc:x} {n}\n")).collect::<String>()).unwrap();
}

/// Interpreter speed alone: the RV spin program, best of 5 runs of 200 frames.
#[test]
#[ignore]
fn bench_rv_spin() {
    if !common::have_tools() {
        return;
    }
    let program = common::program(&common::assemble(RV_SPIN), 4, &[]);
    let us = time_frames(|| Box::new(RvMachine::new(&program)), 200, 5);
    println!("RV spin: {us:.0} us a frame, {:.1} M instructions/s", INSTRUCTIONS_PER_FRAME as f64 / us);
}
