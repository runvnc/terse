//! programs/sparks/sparks.terse: scripted sessions with golden screens (the
//! terminal text and the display list as the host draws it), the display
//! style and fill toggles, determinism, the frame budget, and the image is
//! up to date. Set UPDATE_GOLDEN=1 to rewrite programs/sparks/expected/.
//!
//! `budget` prints the busiest and typical frame (run with `--nocapture`).

mod common;

use std::path::{Path, PathBuf};

use terse_core::{FrameResult, Image, Machine, RAM_BASE, Script};
use terse_net::service::Headless;
use terse_net::{BlobStore, ContentService, Node, Records};

const IMAGE: &[u8] = include_bytes!("../../../programs/sparks/sparks.terse");

fn expected_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/sparks/expected")
}

const RAM_PAGES: u8 = 1;

/// What `terse run --dump --dump-vectors` prints.
fn screen(m: &Machine) -> String {
    let v = m.vector().unwrap();
    let style = v.style_line().map_or(String::new(), |l| l + "\n");
    format!("{}{style}-- vectors: {} list bytes --\n{}", m.terminal().unwrap().text(), v.list().len(), v.dump())
}

struct Session {
    results: Vec<FrameResult>,
    /// Screens after the frames asked for.
    screens: Vec<(u32, String)>,
    list_bytes: Vec<usize>,
    /// The lowest stack pointer reached.
    min_sp: u32,
}

/// The headless run `terse run` makes: a content service on a new, empty
/// store, and named records in `data` (a new directory if `None`; the
/// directories live as long as the returned one).
fn headless_in(script: &str, data: Option<&Path>) -> (Headless, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut node = Node::new(BlobStore::open(dir.path().join("store"), None).unwrap(), None);
    node.set_records(Records::in_data_dir(data.unwrap_or(&dir.path().join("data"))));
    let content = ContentService::start(node);
    (Headless::new(Some(content), Script::parse(script).unwrap()), dir)
}

fn headless(script: &str) -> (Headless, tempfile::TempDir) {
    headless_in(script, None)
}

/// Runs `frames` frames exactly as `terse run` does.
fn session(script: &str, frames: u32, keep: &[u32]) -> Session {
    let mut m = Machine::new(&Image::parse(IMAGE).unwrap());
    let (mut h, _dir) = headless(script);
    let mut s = Session { results: Vec::new(), screens: Vec::new(), list_bytes: Vec::new(), min_sp: 0 };
    for f in 1..=frames {
        let r = h.frame(&mut m);
        assert_eq!(r.stopped, None, "stopped at frame {f}");
        s.results.push(r);
        s.list_bytes.push(m.vector_list().len());
        if keep.contains(&f) {
            s.screens.push((f, screen(&m)));
        }
    }
    s.min_sp = m.cpu().min_sp;
    s
}

fn golden(name: &str, got: &str) {
    let path = expected_dir().join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(expected_dir()).unwrap();
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(want == got, "{name} differs; got:\n{got}");
}

fn sessions_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/sparks/sessions")
}

/// A recorded 3,600-frame session (see `record_sessions`): `keys.txt` with
/// the keyboard, `pad.txt` with controller 0.
fn recorded(name: &str) -> String {
    std::fs::read_to_string(sessions_dir().join(name)).unwrap()
}

/// The number of frames run when the screen first shows `what`.
fn first_frame_showing(script: &str, what: &str, frames: u32) -> u32 {
    let mut m = Machine::new(&Image::parse(IMAGE).unwrap());
    let (mut h, _dir) = headless(script);
    for f in 1..=frames {
        h.frame(&mut m);
        if m.terminal().unwrap().text().contains(what) {
            return f;
        }
    }
    panic!("{what:?} never shown");
}

#[test]
fn keyboard_session() {
    let script = recorded("keys.txt");
    let over = first_frame_showing(&script, "G A M E", 3600);
    let s = session(&script, over, &[30, 900, 1800, over]);
    for (frame, text) in &s.screens {
        golden(&format!("keys-{frame}.txt"), text);
    }
    assert!(s.screens[0].1.contains("demo - Enter or Start to play"));
    assert!(s.screens[1].1.contains("SCORE"));
    assert!(s.screens[3].1.contains("G A M E   O V E R"));
    assert!(!s.screens[3].1.contains("Score 0000"), "the recorded player scores");
}

#[test]
fn controller_session() {
    let script = recorded("pad.txt");
    let s = session(&script, 1800, &[900, 1800]);
    for (frame, text) in &s.screens {
        golden(&format!("pad-{frame}.txt"), text);
    }
    assert!(s.screens[0].1.contains("SCORE"));
    assert!(!s.screens[1].1.contains("SCORE 0000"), "the recorded player scores");
}

/// `sessions/phosphor.txt`: the keyboard session with P pressed (phosphor
/// style, SPEC 10.6), then controller Y twice. The style is the program's
/// state but changes nothing else: the game is the keyboard session's.
#[test]
fn phosphor_session() {
    let script = std::fs::read_to_string(sessions_dir().join("phosphor.txt")).unwrap();
    let mut m = Machine::new(&Image::parse(IMAGE).unwrap());
    let (mut h, _dir) = headless(&script);
    let mut styles = Vec::new();
    for f in 1..=900 {
        h.frame(&mut m);
        if [10, 11, 600, 601, 700, 701].contains(&f) {
            let v = m.vector().unwrap();
            styles.push((f, v.phosphor(), v.persistence()));
        }
    }
    // After f frames have run (frames 0 to f - 1): the press of frame 10 counts from 11 on.
    let want = [(10, false), (11, true), (600, true), (601, false), (700, false), (701, true)];
    assert_eq!(styles, want.map(|(f, p)| (f, p, 160)));
    let got = screen(&m);
    golden("phosphor-900.txt", &got);
    // The shapes are drawn as outlines; the HUD is that of the keyboard
    // session, and so is the list, except that in the phosphor style the
    // arena border and the craft's four lines are drawn twice (overdraw is
    // brighter there, SPEC 10.6).
    let mut k = Machine::new(&Image::parse(IMAGE).unwrap());
    let (mut h, _dir) = headless(&recorded("keys.txt"));
    for _ in 0..900 {
        h.frame(&mut k);
    }
    use terse_core::vector::decode;
    let (phosphor, clean): (Vec<_>, Vec<_>) = (decode(m.vector_list()).collect(), decode(k.vector_list()).collect());
    assert_eq!(phosphor.len(), clean.len() + 8);
    let mut once = phosphor.clone();
    once.drain(4..8);
    once.truncate(once.len() - 4);
    assert_eq!(once, clean);
    assert_eq!(phosphor[..4], phosphor[4..8]);
    assert_eq!(phosphor[phosphor.len() - 8..phosphor.len() - 4], phosphor[phosphor.len() - 4..]);
    assert_eq!(m.terminal().unwrap().text(), k.terminal().unwrap().text());
}

/// `terse run --dump --dump-vectors` prints the same screen.
#[test]
fn cli_dump_matches() {
    let events = sessions_dir().join("keys.txt");
    let image = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/sparks/sparks.terse");
    let dir = tempfile::tempdir().unwrap();
    let out = common::terse_command()
        .args(["run", image.to_str().unwrap(), "--events", events.to_str().unwrap(), "--frames", "900"])
        .args(["--dump", "--dump-vectors"])
        .arg("--store")
        .arg(dir.path().join("store"))
        .arg("--data")
        .arg(dir.path().join("data"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let want = std::fs::read_to_string(expected_dir().join("keys-900.txt")).unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), want);
}

/// State hashes (STATE.md) of the keyboard session at a few frames, golden
/// in `keys-hashes.txt`; `terse run --hash-every` prints the same.
#[test]
fn state_hashes() {
    let frames = [30, 900, 1800, 3000];
    let mut m = Machine::new(&Image::parse(IMAGE).unwrap());
    let (mut h, _dir) = headless(&recorded("keys.txt"));
    let mut got = String::new();
    for f in 1..=3000 {
        h.frame(&mut m);
        if frames.contains(&f) {
            let hash: String = m.state_hash().iter().map(|b| format!("{b:02x}")).collect();
            got += &format!("{f} {hash}\n");
        }
    }
    golden("keys-hashes.txt", &got);

    let events = sessions_dir().join("keys.txt");
    let image = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/sparks/sparks.terse");
    let dir = tempfile::tempdir().unwrap();
    let out = common::terse_command()
        .args(["run", image.to_str().unwrap(), "--events", events.to_str().unwrap(), "--frames", "1800"])
        .args(["--hash-every", "900", "--hash"])
        .arg("--store")
        .arg(dir.path().join("store"))
        .arg("--data")
        .arg(dir.path().join("data"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let lines: Vec<&str> = got.lines().collect();
    let last = lines[2].split_once(' ').unwrap().1;
    assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("{}\n{}\n{last}\n", lines[1], lines[2]));
}

#[test]
fn same_script_same_run() {
    let script = recorded("keys.txt");
    let keep: Vec<u32> = (1..=12).map(|i| i * 100).collect();
    let a = session(&script, 1200, &keep);
    let b = session(&script, 1200, &keep);
    assert_eq!(a.screens, b.screens);
    assert_eq!(a.results, b.results);
}

/// F (or controller X) turns the fill off: filled shapes are drawn as
/// outlines in the clean style. The game and its display list are the same.
#[test]
fn fill_toggle() {
    let run = |script: &str| {
        let mut m = Machine::new(&Image::parse(IMAGE).unwrap());
        let (mut h, _dir) = headless(script);
        let mut fills = Vec::new();
        for _ in 0..60 {
            h.frame(&mut m);
            fills.push(m.vector().unwrap().fill());
        }
        (m, fills)
    };
    let (plain, _) = run("");
    let (off, fills) = run("10 press f\n");
    // After 11 frames (0-10) the press of frame 10 has been read.
    assert!(fills[..10].iter().all(|&f| f) && fills[10..].iter().all(|&f| !f));
    assert_eq!(off.vector_list(), plain.vector_list(), "the same list, drawn differently");
    let got = screen(&off);
    assert!(got.contains("-- vector style: clean, fill off --\n"), "{got}");
    let shapes = got.split_once(" list bytes --\n").unwrap().1;
    assert!(shapes.lines().all(|l| l.starts_with("line ") || l.starts_with("circle ")), "{shapes}");
    assert!(screen(&plain).contains("\ndisc "), "the sparks are discs with the fill on");
    assert_eq!(off.terminal().unwrap().text(), plain.terminal().unwrap().text());

    // F again, and controller X twice: on, off, on.
    let (m, fills) = run("10 press f\n20 press f\n30 pad 0 40 0 0\n31 pad 0 0 0 0\n40 pad 0 40 0 0\n41 pad 0 0 0 0\n");
    assert_eq!([fills[10], fills[20], fills[30], fills[40]], [false, true, false, true]);
    assert_eq!(screen(&m), screen(&plain));
}

/// Both recorded sessions, 3,600 frames each: every frame's work ends in
/// wait_frame within its budget.
#[test]
fn budget() {
    for name in ["keys.txt", "pad.txt"] {
        let start = std::time::Instant::now();
        let s = session(&recorded(name), 3600, &[]);
        let host = start.elapsed();
        let spilled = s.results.iter().filter(|r| !r.waited).count();
        let used: Vec<u64> = s.results.iter().map(|r| r.used).collect();
        let stats = common::FrameStats::new(&used);
        let after_reset = common::FrameStats::new(&used[1..]);
        // Until the recording starts a game (frame 60), the demo runs with
        // six sweepers and eight sparks.
        let demo = common::FrameStats::new(&used[1..60]);
        let mut bytes = s.list_bytes.clone();
        bytes.sort_unstable();
        eprintln!(
            "sparks {name}: {}\n  after frame 0: busiest {} units ({:.2}%); demo busiest {} units ({:.2}%); \
             list bytes median {} max {}; deepest stack {} bytes; frames not ending in wait_frame: {spilled}; \
             host {:.1} µs per frame",
            stats.describe(),
            after_reset.busiest,
            common::pct(after_reset.busiest),
            demo.busiest,
            common::pct(demo.busiest),
            bytes[bytes.len() / 2],
            bytes[bytes.len() - 1],
            RAM_BASE + RAM_PAGES as u32 * 8192 - s.min_sp,
            host.as_secs_f64() * 1e6 / 3600.0,
        );
        assert_eq!(spilled, 0, "{name}: a frame's work ran into the next frame");
        assert!(common::pct(stats.busiest) < 50.0, "{name}: busiest frame {} units", stats.busiest);
    }
}

/// SPEC 9.6: the high score is the named record "high-score", saved when a
/// game beats it and loaded at start, without delaying the first frame. A
/// second run with the same data directory shows the first run's score.
#[test]
fn high_score_persists() {
    let data = tempfile::tempdir().unwrap();
    let script = recorded("keys.txt");
    let over = first_frame_showing(&script, "G A M E", 3600);
    let mut m = Machine::new(&Image::parse(IMAGE).unwrap());
    let (mut h, _dir) = headless_in(&script, Some(data.path()));
    let first = h.frame(&mut m);
    assert!(first.waited, "frame 0 ends in wait_frame: the load does not hold it up");
    for _ in 1..over + 2 {
        h.frame(&mut m);
    }
    let text = m.terminal().unwrap().text();
    assert!(text.contains("New high score!"), "{text}");
    let score = text.split("Score ").nth(1).unwrap()[..4].to_string();
    assert_ne!(score, "0000");
    let owner = terse_net::Owner::Image(Image::parse(IMAGE).unwrap().id);
    let saved = Records::in_data_dir(data.path()).load(&owner, b"high-score").unwrap().unwrap();
    assert_eq!(u32::from_le_bytes(saved[..].try_into().unwrap()), score.parse::<u32>().unwrap());

    // The second run: no input; the high score arrives in frame 1.
    let mut m = Machine::new(&Image::parse(IMAGE).unwrap());
    let (mut h, _dir) = headless_in("", Some(data.path()));
    h.frame(&mut m);
    assert!(m.terminal().unwrap().text().contains("HIGH 0000"));
    for _ in 0..29 {
        h.frame(&mut m);
    }
    let text = m.terminal().unwrap().text();
    assert!(text.contains(&format!("HIGH {score}")), "{text}");
    golden("high-30.txt", &screen(&m));
    // Without the record (another data directory) it is 0000.
    let mut m = Machine::new(&Image::parse(IMAGE).unwrap());
    let (mut h, _dir) = headless("");
    for _ in 0..30 {
        h.frame(&mut m);
    }
    assert!(m.terminal().unwrap().text().contains("HIGH 0000"));
}

#[test]
fn image_is_up_to_date() {
    common::check_up_to_date(IMAGE, &["sparks/sparks.c"], true, "sparks", RAM_PAGES, &[(2, 0), (3, 0), (4, 0x80)]);
}

/// Records programs/sparks/sessions/{keys,pad}.txt: a player that reads only
/// the screen (the display list and the terminal text) and steers for the
/// nearest spark away from the sweepers. Run with
/// `cargo test -p terse-cli --test sparks record_sessions -- --ignored`, then
/// rewrite the screens with UPDATE_GOLDEN=1.
#[test]
#[ignore]
fn record_sessions() {
    std::fs::create_dir_all(sessions_dir()).unwrap();
    for (name, pad) in [("keys.txt", false), ("pad.txt", true)] {
        // The keyboard player stops minding the sweepers late on, so that the
        // session ends a game and starts another.
        let script = bot::record(IMAGE, pad, 3600, if pad { 3600 } else { 2000 });
        // The recording replays exactly.
        let a = session(&script, 3600, &[3600]);
        assert!(!a.screens[0].1.is_empty());
        std::fs::write(sessions_dir().join(name), script).unwrap();
    }
}

mod bot {
    use terse_core::script::key_code;
    use terse_core::vector::{Primitive, decode};
    use terse_core::{CONTROLLERS, Controller, Event, FrameInput, Image, Machine, pad};

    const SWEEPER: [u8; 4] = [0xFF, 0x50, 0x32, 0xFF];
    const SPARK_RADIUS: u16 = 52;

    #[derive(Clone, Copy, Default, PartialEq)]
    struct Held {
        left: bool,
        right: bool,
        thrust: bool,
        start: bool,
    }

    type P = (f32, f32);

    struct Seen {
        nose: P,
        back: P,
        sparks: Vec<P>,
        /// Pivot and half length.
        sweepers: Vec<(P, f32)>,
    }

    fn dist(a: P, b: P) -> f32 {
        (a.0 - b.0).hypot(a.1 - b.1)
    }

    fn look(list: &[u8]) -> Option<Seen> {
        let mut seen = Seen { nose: (0.0, 0.0), back: (0.0, 0.0), sparks: vec![], sweepers: vec![] };
        let mut craft = false;
        for p in decode(list) {
            match p {
                Primitive::Disc { x, y, radius: SPARK_RADIUS, .. } => seen.sparks.push((x as f32, y as f32)),
                Primitive::Line { x0, y0, x1, y1, rgba: SWEEPER, .. } => {
                    let (a, b) = ((x0 as f32, y0 as f32), (x1 as f32, y1 as f32));
                    seen.sweepers.push((((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0), dist(a, b) / 2.0));
                }
                Primitive::Triangle { x0, y0, x2, y2, .. } if !craft => {
                    craft = true;
                    seen.nose = (x0 as f32, y0 as f32);
                    seen.back = (x2 as f32, y2 as f32);
                }
                _ => {}
            }
        }
        craft.then_some(seen)
    }

    /// What to hold this frame, given the screen and the craft's last position.
    fn steer(seen: &Seen, last: &mut Option<P>, careful: bool) -> Held {
        let pos = seen.back;
        let vel = last.map_or((0.0, 0.0), |l| (pos.0 - l.0, pos.1 - l.1));
        *last = Some(pos);
        let head = (seen.nose.0 - seen.back.0, seen.nose.1 - seen.back.1);
        let ahead = (pos.0 + vel.0 * 14.0, pos.1 + vel.1 * 14.0);
        // Danger: drifting into a sweeper's circle.
        let danger = seen
            .sweepers
            .iter()
            .filter(|&&(c, len)| careful && dist(ahead, c) < len + 260.0)
            .min_by(|a, b| dist(pos, a.0).total_cmp(&dist(pos, b.0)));
        let (want, escaping) = match danger {
            Some(&(c, _)) => ((pos.0 - c.0, pos.1 - c.1), true),
            None => {
                let safe = |s: &&P| !careful || seen.sweepers.iter().all(|&(c, len)| dist(**s, c) > len + 120.0);
                let near = |a: &&P, b: &&P| dist(**a, pos).total_cmp(&dist(**b, pos));
                let target = seen.sparks.iter().filter(safe).min_by(near).or_else(|| seen.sparks.iter().min_by(near));
                match target {
                    // Aim where the spark is relative to where the drift takes us.
                    Some(t) => ((t.0 - pos.0 - vel.0 * 10.0, t.1 - pos.1 - vel.1 * 10.0), false),
                    None => return Held::default(),
                }
            }
        };
        let cross = head.0 * want.1 - head.1 * want.0;
        let dot = head.0 * want.0 + head.1 * want.1;
        let cos = dot / (head.0.hypot(head.1) * want.0.hypot(want.1)).max(1e-3);
        let speed = vel.0.hypot(vel.1);
        Held {
            left: cross < 0.0 && cos < 0.97,
            right: cross > 0.0 && cos < 0.97,
            thrust: cos > 0.8 && (escaping || speed < 45.0),
            start: false,
        }
    }

    /// Runs the game with the player for `frames` frames; returns its input
    /// as a script. From frame `careless_from` on it ignores the sweepers.
    pub fn record(image: &[u8], use_pad: bool, frames: u32, careless_from: u32) -> String {
        let mut m = Machine::new(&Image::parse(image).unwrap());
        let mut out = format!(
            "# Recorded by record_sessions (crates/terse-cli/tests/sparks.rs) with {}.\n",
            if use_pad { "controller 0" } else { "the keyboard" }
        );
        let (mut held, mut last, mut over_since) = (Held::default(), None, None);
        let mut controllers = [Controller::default(); CONTROLLERS];
        for f in 0..frames {
            let text = m.terminal().unwrap().text();
            let mut want = Held::default();
            if text.contains("demo - Enter") {
                want.start = f == 60;
            } else if text.contains("G A M E") {
                let since = *over_since.get_or_insert(f);
                want.start = f == since + 90;
            } else if f > 0 {
                over_since = None;
                if let Some(seen) = look(m.vector_list()) {
                    want = steer(&seen, &mut last, f < careless_from);
                }
            }
            let mut events = Vec::new();
            if !use_pad {
                for (was, now, key) in
                    [(held.left, want.left, "left"), (held.right, want.right, "right"), (held.thrust, want.thrust, "up")]
                {
                    if was != now {
                        out += &format!("{f} {} {key}\n", if now { "down" } else { "up" });
                        let code = key_code(key).unwrap();
                        events.push(if now { Event::key_down(code, 0) } else { Event::key_up(code, 0) });
                    }
                }
                if want.start {
                    out += &format!("{f} press enter\n");
                    events.extend([Event::key_down(0x28, 0), Event::key_up(0x28, 0)]);
                }
            } else if want != held || f == 0 {
                // Turn with the stick, thrust with A, start with Start.
                let lx: i8 = if want.left { -100 } else if want.right { 100 } else { 0 };
                let buttons = if want.thrust { pad::A } else { 0 } | if want.start { pad::START } else { 0 };
                out += &format!("{f} pad 0 {buttons:x} {lx} 0\n");
                controllers[0] = Controller { connected: true, buttons, left: (lx, 0), right: (0, 0) };
            }
            held = want;
            m.run_frame(&FrameInput { events: &events, controllers, ..Default::default() });
        }
        out
    }
}
