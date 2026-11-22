//! programs/reef/reef.terse (Lumen Reef): a recorded session from the title
//! through the stages to the game over, with golden screens (both display
//! lists as the host draws them) and state hashes; the frame budget and
//! both displays' ink caps over long sessions with and without a player;
//! the devices it needs and the ones it can do without; and the image is
//! up to date. Set UPDATE_GOLDEN=1 to rewrite programs/reef/expected/.
//!
//! `limits` prints the busiest frame and the most ink (run with `--nocapture`).

mod common;

use std::path::{Path, PathBuf};

use terse_core::{FrameResult, Image, Machine, RAM_BASE, Script};
use terse_net::service::Headless;
use terse_net::{BlobStore, ContentService, Node, Records};

/// The image, read when a test starts (so that a rebuilt image needs no
/// rebuilt test).
static IMAGE_BYTES: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| std::fs::read(dir().join("reef.terse")).unwrap());
fn image() -> Image {
    Image::parse(&IMAGE_BYTES).unwrap()
}
const RAM_PAGES: u8 = 3;
/// The display lists' buffers in reef.c (list2, list3).
const LIST2_BYTES: usize = 6144;
const LIST3_BYTES: usize = 12288;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/reef")
}

fn golden(name: &str, got: &str) {
    common::golden(&format!("reef/expected/{name}"), got);
}

/// What `terse run --dump --dump-vectors` prints: the 2D list as drawn,
/// then the 3D list with what the reference eye sees.
fn screen(m: &Machine) -> String {
    let v = m.vector().unwrap();
    let style = v.style_line().map_or(String::new(), |l| l + "\n");
    format!("{style}-- vectors: {} list bytes --\n{}{}", v.list().len(), v.dump(), m.vector3().unwrap().dump())
}

/// The headless run `terse run` makes, with a content service on a new,
/// empty store and named records in `data` (a new directory if `None`).
fn headless_in(script: &str, data: Option<&Path>) -> (Headless, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut node = Node::new(BlobStore::open(dir.path().join("store"), None).unwrap(), None);
    node.set_records(Records::in_data_dir(data.unwrap_or(&dir.path().join("data"))));
    let content = ContentService::start(node);
    (Headless::new(Some(content), Script::parse(script).unwrap()), dir)
}

/// The game's state as reef.c keeps it (`struct game`, found by its first
/// eight bytes, "REEFSTAT").
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Status {
    state: u8,
    stage: u8,
    shields: u8,
    paused: u8,
    score: u32,
    high: u32,
    kills: u16,
    shots: u16,
    hits: u16,
    aim: (i16, i16),
    heat: u16,
    kind: u8,
    nodes: u8,
}

const TITLE: u8 = 0;
const FIGHT: u8 = 2;
const OVER: u8 = 5;

#[derive(Clone, Copy, Debug)]
struct Obj {
    kind: u8,
    x: i32,
    y: i32,
    z: i32,
}

/// Object types (reef.c).
const KITE: u8 = 1;
const PRONG: u8 = 3;
const SPARK: u8 = 4;
const PILLAR: u8 = 5;
const SLAB: u8 = 6;
const GATE: u8 = 7;
const NODE: u8 = 8;
const CORE: u8 = 9;
const MAXOBJ: usize = 28;

fn find_game(ram: &[u8]) -> usize {
    ram.windows(8).position(|w| w == b"REEFSTAT").expect("the game's state in RAM")
}

fn status(ram: &[u8], at: usize) -> Status {
    let b = &ram[at..];
    let u16_ = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let u32_ = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    Status {
        state: b[8],
        stage: b[9],
        shields: b[10],
        paused: b[11],
        score: u32_(12),
        high: u32_(16),
        kills: u16_(20),
        shots: u16_(24),
        hits: u16_(26),
        aim: (u16_(28) as i16, u16_(30) as i16),
        heat: u16_(32),
        kind: b[36],
        nodes: b[39],
    }
}

fn objects(ram: &[u8], at: usize) -> Vec<Obj> {
    (0..MAXOBJ)
        .map(|i| {
            let o = &ram[at + 44 + i * 28..];
            let i32_ = |k: usize| i32::from_le_bytes(o[k..k + 4].try_into().unwrap());
            Obj { kind: o[0], x: i32_(4), y: i32_(8), z: i32_(12) }
        })
        .filter(|o| o.kind != 0)
        .collect()
}

struct Session {
    results: Vec<FrameResult>,
    screens: Vec<(u32, String)>,
    statuses: Vec<Status>,
    list2: Vec<usize>,
    list3: Vec<usize>,
    /// Ink of each frame's 2D and 3D lists (square screen units), and whether
    /// a 3D list was capped.
    ink2: Vec<u64>,
    ink3: Vec<f64>,
    capped3: usize,
    min_sp: u32,
}

fn ink2(list: &[u8]) -> u64 {
    terse_core::vector::decode(list).map(|p| terse_core::vector::ink(&p)).sum()
}

/// Runs `frames` frames as `terse run` does.
fn session(script: &str, frames: u32, keep: &[u32], measure: bool) -> Session {
    let mut m = Machine::new(&image());
    let (mut h, _dir) = headless_in(script, None);
    let mut s = Session {
        results: vec![],
        screens: vec![],
        statuses: vec![],
        list2: vec![],
        list3: vec![],
        ink2: vec![],
        ink3: vec![],
        capped3: 0,
        min_sp: 0,
    };
    let mut at = None;
    for f in 1..=frames {
        let r = h.frame(&mut m);
        assert_eq!(r.stopped, None, "stopped at frame {f}");
        s.results.push(r);
        let at = *at.get_or_insert_with(|| find_game(m.ram()));
        s.statuses.push(status(m.ram(), at));
        if measure {
            let (l2, l3) = (m.vector_list(), m.vector3().unwrap().list());
            s.list2.push(l2.len());
            s.list3.push(l3.len());
            s.ink2.push(ink2(l2));
            let p = terse_core::vector3::project(l3, (0.0, 0.0), &mut Vec::new());
            s.ink3.push(p.ink);
            s.capped3 += p.capped as usize;
        }
        if keep.contains(&f) {
            s.screens.push((f, screen(&m)));
        }
    }
    s.min_sp = m.cpu().min_sp;
    s
}

fn recorded(name: &str) -> String {
    std::fs::read_to_string(dir().join("sessions").join(name)).unwrap()
}

/// The first frame (counted from 1) at which `want` holds.
fn first(s: &Session, want: impl Fn(&Status) -> bool) -> Option<u32> {
    s.statuses.iter().position(want).map(|i| i as u32 + 1)
}

/// `sessions/play.txt`, recorded by `record_sessions`: the title, a press
/// of Space, then a player aiming with the pointer through the stages
/// until it is careless and loses its shields, the game over and a second
/// start. Golden screens at a few points, and what happened.
#[test]
fn recorded_session() {
    let script = recorded("play.txt");
    let all = session(&script, 7200, &[], false);
    let over = first(&all, |s| s.state == OVER).expect("the game ends");
    let warden = first(&all, |s| s.kind == 2 && s.state == FIGHT).expect("a Warden");
    let run = first(&all, |s| s.kind == 1 && s.state == FIGHT).expect("a reef run");
    let keep = [60, run + 300, warden + 240, over + 90];
    let s = session(&script, over + 90, &keep, false);
    let names = ["title", "run", "warden", "over"];
    for ((_, text), name) in s.screens.iter().zip(names) {
        golden(&format!("play-{name}.txt"), text);
    }
    let end = all.statuses[over as usize - 1];
    eprintln!("reef play.txt: run at {run}, Warden at {warden}, over at {over}: {end:?}");
    assert_eq!(all.statuses[59].state, TITLE);
    assert!(end.stage >= 3, "the player reaches the Warden: {end:?}");
    assert!(end.score >= 3000 && end.hits > 0 && end.shots > end.hits, "{end:?}");
    // Played again after the game over.
    assert!(all.statuses[over as usize + 400].state != OVER && all.statuses[over as usize + 400].state != TITLE);
}

/// State hashes (STATE.md) of the recorded session, golden; `terse run
/// --hash-every` prints the same.
#[test]
fn state_hashes() {
    let frames = [60, 1200, 2400, 3600];
    let mut m = Machine::new(&image());
    let (mut h, _dir) = headless_in(&recorded("play.txt"), None);
    let mut got = String::new();
    for f in 1..=3600 {
        h.frame(&mut m);
        if frames.contains(&f) {
            let hash: String = m.state_hash().iter().map(|b| format!("{b:02x}")).collect();
            got += &format!("{f} {hash}\n");
        }
    }
    golden("play-hashes.txt", &got);
    let events = dir().join("sessions/play.txt");
    let image = dir().join("reef.terse");
    let tmp = tempfile::tempdir().unwrap();
    let out = common::terse_command()
        .args(["run", image.to_str().unwrap(), "--events", events.to_str().unwrap(), "--frames", "2400"])
        .args(["--hash-every", "1200", "--hash", "--relay", "none"])
        .arg("--store")
        .arg(tmp.path().join("store"))
        .arg("--data")
        .arg(tmp.path().join("data"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let lines: Vec<&str> = got.lines().collect();
    let last = lines[2].split_once(' ').unwrap().1;
    assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("{}\n{}\n{last}\n", lines[1], lines[2]));
}

/// The same input gives the same run.
#[test]
fn same_script_same_run() {
    let script = recorded("play.txt");
    let keep: Vec<u32> = (1..=10).map(|i| i * 150).collect();
    let a = session(&script, 1500, &keep, false);
    let b = session(&script, 1500, &keep, false);
    assert_eq!(a.screens, b.screens);
    assert_eq!(a.results, b.results);
}

/// Long sessions, every frame: its work ends in wait_frame well inside the
/// budget, and both lists stay well under their ink caps (so the caps never
/// cut a picture) and fit their buffers. Sessions: the recorded player; a
/// start and no input after it (the skiff is hit until the game is over,
/// then the game-over screen waits); held fire with the keys sweeping the
/// sight; and the controller.
#[test]
fn limits() {
    let sweep = {
        let mut s = String::from("30 press space\n150 down space\n");
        for i in 0..40 {
            let f = 160 + i * 120;
            let (a, b) = if i % 2 == 0 { ("right", "up") } else { ("left", "down") };
            s += &format!("{f} down {a}\n{} up {a}\n{} down {b}\n{} up {b}\n", f + 50, f + 60, f + 90);
        }
        s
    };
    let pad = "30 pad 0 10 0 0\n31 pad 0 0 0 0\n200 pad 0 10 100 -60\n400 pad 0 10 -100 50\n700 pad 0 10 0 0\n";
    let play = recorded("play.txt");
    let runs: [(&str, &str, u32); 4] = [("play.txt", &play, 7200), ("idle", "30 press space\n", 5400), ("sweep", &sweep, 5400), ("pad", pad, 3000)];
    let (mut worst_units, mut worst2, mut worst3) = (0u64, 0u64, 0f64);
    for (name, script, frames) in runs {
        let s = session(script, frames, &[], true);
        let spilled = s.results.iter().filter(|r| !r.waited).count();
        let used: Vec<u64> = s.results.iter().map(|r| r.used).collect();
        let stats = common::FrameStats::new(&used[1..]);
        let (i2, i3) = (*s.ink2.iter().max().unwrap(), s.ink3.iter().cloned().fold(0.0, f64::max));
        let (l2, l3) = (*s.list2.iter().max().unwrap(), *s.list3.iter().max().unwrap());
        let reached = s.statuses.iter().map(|s| s.stage).max().unwrap();
        eprintln!(
            "reef {name}, {frames} frames: {}\n  most ink: 2D {i2} ({:.1}% of 230,400), 3D {i3:.0} ({:.1}% of 153,600); \
             list bytes: 2D {l2}, 3D {l3}; deepest stack {} bytes; reached stage {}",
            stats.describe(),
            i2 as f64 * 100.0 / 230_400.0,
            i3 * 100.0 / 153_600.0,
            RAM_BASE + RAM_PAGES as u32 * 8192 - s.min_sp,
            reached + 1,
        );
        assert_eq!(spilled, 0, "{name}: a frame's work ran into the next frame");
        assert_eq!(s.capped3, 0, "{name}: a 3D list reached a cap");
        assert!(l2 < LIST2_BYTES && l3 < LIST3_BYTES, "{name}: lists {l2}, {l3}");
        worst_units = worst_units.max(stats.busiest);
        worst2 = worst2.max(i2);
        worst3 = worst3.max(i3);
    }
    eprintln!("reef worst: {worst_units} units ({:.1}%), 2D ink {worst2}, 3D ink {worst3:.0}", common::pct(worst_units));
    assert!(common::pct(worst_units) < 50.0, "busiest frame {worst_units} units");
    assert!((worst2 as f64) < 0.7 * 230_400.0, "2D ink {worst2}");
    assert!(worst3 < 0.7 * 153_600.0, "3D ink {worst3}");
}

/// The panels and their text: the 2D list on the title and in play; and
/// pause holds the game (P, then P again).
#[test]
fn pause_holds() {
    let script = "30 press space\n300 press p\n";
    let a = session(script, 600, &[], false);
    let paused = &a.statuses[310..600];
    assert!(paused.iter().all(|s| s.paused == 1 && *s == paused[0]), "nothing moves while paused");
    let b = session("30 press space\n300 press p\n400 press p\n", 600, &[], false);
    assert_eq!(b.statuses[450].paused, 0);
    assert_ne!(b.statuses[450], b.statuses[599]);
}

/// Device 9 and device 3 are required: a run that does not provide 9 is
/// refused. Sound (6), content (4) and the terminal (2) are optional: the
/// game is the same without them.
#[test]
fn devices() {
    common::check_up_to_date(&IMAGE_BYTES, &["reef/reef.c"], true, "reef", RAM_PAGES, &[(3, 0), (9, 0), (2, 0x80), (4, 0x80), (6, 0x80)]);
    let image = dir().join("reef.terse");
    let tmp = tempfile::tempdir().unwrap();
    let run = |extra: &[&str]| {
        common::terse_command()
            .args(["run", image.to_str().unwrap(), "--frames", "900", "--hash", "--relay", "none"])
            .arg("--events")
            .arg(dir().join("sessions/play.txt"))
            .arg("--store")
            .arg(tmp.path().join("store"))
            .arg("--data")
            .arg(tmp.path().join("data"))
            .args(extra)
            .output()
            .unwrap()
    };
    for slot in ["9", "3"] {
        let out = run(&["--without", slot]);
        assert!(!out.status.success(), "without device {slot}: refused");
        eprintln!("without device {slot}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    for slot in ["6", "4", "2"] {
        let out = run(&["--without", slot]);
        assert!(out.status.success(), "without device {slot}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

/// Sound: firing and explosions make the frames' samples; the sound device
/// is optional, so the same game runs silent without it (`devices`).
#[test]
fn sound() {
    let mut m = Machine::new(&image());
    let (mut h, _dir) = headless_in("30 press space\n160 down space\n", None);
    let mut loud = 0;
    for _ in 0..400 {
        h.frame(&mut m);
        if let Some(samples) = m.take_audio() {
            loud += samples.iter().any(|&s| s.unsigned_abs() > 500) as u32;
        }
    }
    assert!(loud > 50, "{loud} frames with sound");
}

#[test]
fn image_is_up_to_date() {
    common::check_up_to_date(&IMAGE_BYTES, &["reef/reef.c"], true, "reef", RAM_PAGES, &[(3, 0), (9, 0), (2, 0x80), (4, 0x80), (6, 0x80)]);
}

/// Records programs/reef/sessions/play.txt: a player that reads the game's
/// state from RAM and aims with the pointer (with its button held to fire),
/// dodging sparks and the reef's slabs and pillars, flying through gates;
/// from frame 5000 it stops dodging and firing, so the game ends, and it
/// starts another. Run with `cargo test -p terse-cli --test reef
/// record_sessions -- --ignored`, then rewrite the goldens with UPDATE_GOLDEN=1.
#[test]
#[ignore]
fn record_sessions() {
    std::fs::create_dir_all(dir().join("sessions")).unwrap();
    let script = bot::record(&IMAGE_BYTES, 7200, 5000);
    std::fs::write(dir().join("sessions/play.txt"), &script).unwrap();
    let s = session(&script, 7200, &[], false);
    let mut last = Status::default();
    for (f, st) in s.statuses.iter().enumerate() {
        if (st.state, st.stage, st.shields) != (last.state, last.stage, last.shields) {
            eprintln!("{}: state {} stage {} shields {} score {} shots {} hits {}", f + 1, st.state, st.stage + 1, st.shields, st.score, st.shots, st.hits);
        }
        last = *st;
    }
}

mod bot {
    use super::*;
    use terse_core::{Event, FrameInput};

    fn sign(v: i32) -> i32 {
        if v < 0 { -1 } else { 1 }
    }

    /// Where the player puts the sight this frame, and whether it fires.
    fn decide(s: &Status, objs: &[Obj], firing: &mut bool, careful: bool) -> (i32, i32) {
        let u = |v: i32| v / 16;
        let proj = |o: &Obj| (u(o.x) * 960 / (960 + u(o.z)), u(o.y) * 960 / (960 + u(o.z)));
        // Heat: fire up to near the top of the gauge, then let it cool.
        if s.heat > 330 {
            *firing = false;
        } else if s.heat < 150 {
            *firing = true;
        }
        if !careful {
            *firing = false;
            return (0, 0);
        }
        let (mut ax, mut ay) = (None, None);
        for o in objs {
            let (x, y, z) = (u(o.x), u(o.y), u(o.z));
            match o.kind {
                SPARK if z < 600 && x.abs() < 90 && y.abs() < 80 => {
                    ax = Some(-sign(x) * 230);
                    ay = Some(-sign(y) * 150);
                }
                PILLAR if z < 1100 && x.abs() < 120 => ax = Some(-sign(x) * 230),
                SLAB if z < 1100 && y.abs() < 90 => ay = Some(-sign(y) * 150),
                PRONG if z < 500 && x.abs() < 90 && y.abs() < 80 => {
                    ax = ax.or(Some(-sign(x) * 230));
                    ay = ay.or(Some(-sign(y) * 150));
                }
                _ => {}
            }
        }
        // Otherwise the nearest target, or gate.
        let target = objs
            .iter()
            .filter(|o| matches!(o.kind, KITE..=PRONG | NODE | GATE) || (o.kind == CORE && s.nodes == 0))
            .filter(|o| o.z > 150 * 16)
            .min_by_key(|o| if o.kind == GATE { o.z + 4000 * 16 } else { o.z });
        let (tx, ty) = target.map_or((0, 0), proj);
        (ax.unwrap_or(tx), ay.unwrap_or(ty))
    }

    /// Runs the game with the player for `frames` frames; returns its input
    /// as a script.
    pub fn record(image: &[u8], frames: u32, careless_from: u32) -> String {
        let mut m = Machine::new(&Image::parse(image).unwrap());
        let mut out = String::from("# Recorded by record_sessions (crates/terse-cli/tests/reef.rs): the pointer aims, its button fires.\n");
        let mut at = None;
        let (mut over_since, mut firing, mut last) = (None, true, (320u16, 240u16, 0u8));
        for f in 0..frames {
            let mut events = Vec::new();
            if f > 0 {
                let at = *at.get_or_insert_with(|| find_game(m.ram()));
                let s = status(m.ram(), at);
                let press = |events: &mut Vec<Event>, out: &mut String| {
                    *out += &format!("{f} press space\n");
                    events.extend([Event::key_down(0x2C, 0), Event::key_up(0x2C, 0)]);
                };
                if s.state == TITLE {
                    if f == 60 {
                        press(&mut events, &mut out);
                    }
                } else if s.state == OVER {
                    let since = *over_since.get_or_insert(f);
                    if f == since + 120 {
                        press(&mut events, &mut out);
                    }
                } else {
                    over_since = None;
                    let (x, y) = decide(&s, &objects(m.ram(), at), &mut firing, f < careless_from);
                    let now = ((320 + x).clamp(0, 639) as u16, (240 + y).clamp(0, 479) as u16, firing as u8);
                    if now != last {
                        out += &format!("{f} pointer {} {} {}\n", now.0, now.1, now.2);
                        events.push(Event::pointer(now.0, now.1, now.2));
                        last = now;
                    }
                }
            }
            m.run_frame(&FrameInput { events: &events, ..Default::default() });
        }
        out
    }
}
