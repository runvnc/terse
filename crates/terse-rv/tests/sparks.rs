//! programs/rv/sparks/sparks.terse (profile 2) against the profile-1 game:
//! the recorded sessions of programs/sparks/sessions/ replayed, compared
//! with the same golden screens (terminal text and decoded display list) in
//! programs/sparks/expected/, determinism, the image is up to date, and the
//! budget (`budget` prints the measurements; run with `--nocapture`).

mod common;

use std::path::{Path, PathBuf};

use terse_core::{FrameMachine, FrameResult, Script};
use terse_net::service::Headless;
use terse_rv::{INSTRUCTIONS_PER_FRAME, RvMachine};

const IMAGE: &[u8] = include_bytes!("../../../programs/rv/sparks/sparks.terse");

fn sparks_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/sparks")
}

fn machine() -> RvMachine {
    RvMachine::new(&terse_rv::parse_image(IMAGE).unwrap())
}

/// What `terse run --dump --dump-vectors` prints.
fn screen(m: &RvMachine) -> String {
    let list = m.vector_list();
    format!("{}-- vectors: {} list bytes --\n{}", m.terminal().unwrap().text(), list.len(), terse_core::vector::dump(list))
}

struct Session {
    results: Vec<FrameResult>,
    screens: Vec<(u32, String)>,
    list_bytes: Vec<usize>,
    min_sp: u32,
}

fn session(script: &str, frames: u32, keep: &[u32]) -> Session {
    let mut m = machine();
    let mut h = Headless::new(None, Script::parse(script).unwrap());
    let mut s = Session { results: Vec::new(), screens: Vec::new(), list_bytes: Vec::new(), min_sp: 0 };
    for f in 1..=frames {
        let r = h.frame(&mut m);
        assert_eq!(r.exit, None, "exited at frame {f}: {:?}", m.fault());
        s.results.push(r);
        s.list_bytes.push(m.vector_list().len());
        if keep.contains(&f) {
            s.screens.push((f, screen(&m)));
        }
    }
    s.min_sp = m.cpu().min_sp;
    s
}

fn recorded(name: &str) -> String {
    std::fs::read_to_string(sparks_dir().join("sessions").join(name)).unwrap()
}

fn expected(name: &str) -> String {
    std::fs::read_to_string(sparks_dir().join("expected").join(name)).unwrap()
}

/// The profile-1 game's golden screens: the frames are those of
/// crates/terse-cli/tests/sparks.rs (the keyboard session ends its game at
/// frame 2,999).
#[test]
fn keyboard_session_matches_the_z80_game() {
    let s = session(&recorded("keys.txt"), 2999, &[30, 900, 1800, 2999]);
    for (frame, text) in &s.screens {
        assert!(*text == expected(&format!("keys-{frame}.txt")), "keys-{frame} differs; got:\n{text}");
    }
}

#[test]
fn controller_session_matches_the_z80_game() {
    let s = session(&recorded("pad.txt"), 1800, &[900, 1800]);
    for (frame, text) in &s.screens {
        assert!(*text == expected(&format!("pad-{frame}.txt")), "pad-{frame} differs; got:\n{text}");
    }
}

#[test]
fn same_script_same_run() {
    let script = recorded("keys.txt");
    let keep: Vec<u32> = (1..=6).map(|i| i * 100).collect();
    let a = session(&script, 600, &keep);
    let b = session(&script, 600, &keep);
    assert_eq!(a.screens, b.screens);
    assert_eq!(a.results, b.results);
}

#[test]
fn image_is_up_to_date() {
    if !common::have_tools() {
        return;
    }
    let rom = common::build(&[common::workspace().join("programs/rv/sparks/sparks.c")], "-Os", true);
    let image = terse_rv::parse_image(IMAGE).unwrap();
    assert!(rom == image.rom, "programs/rv/sparks/sparks.terse is out of date: run programs/rv/sparks/build.sh");
    assert_eq!(image.title, b"sparks");
    assert_eq!(image.ram_pages, 4);
}

/// Both recorded sessions, 3,600 frames each: every frame's work ends in
/// wait_frame within its budget.
#[test]
fn budget() {
    for name in ["keys.txt", "pad.txt"] {
        let s = session(&recorded(name), 3600, &[]);
        let spilled = s.results.iter().filter(|r| !r.halted_early).count();
        let mut t: Vec<u64> = s.results.iter().map(|r| r.tstates).collect();
        let (worst_frame, worst) = t.iter().copied().enumerate().max_by_key(|&(_, v)| v).unwrap();
        let after_reset = t[1..].iter().max().copied().unwrap();
        let demo_worst = t[1..60].iter().max().copied().unwrap();
        t.sort_unstable();
        let median = t[t.len() / 2];
        let pct = |v: u64| v as f64 * 100.0 / INSTRUCTIONS_PER_FRAME as f64;
        let ram_end = terse_rv::RAM_BASE + 4 * 8192;
        eprintln!(
            "sparks-rv {name}: worst frame {worst_frame}: {worst} instructions ({:.2}%); worst after frame 0: \
             {after_reset} ({:.2}%); demo (6 sweepers, 8 sparks) worst {demo_worst} ({:.2}%); median {median} \
             ({:.2}%); deepest stack {} bytes; frames not ending in wait_frame: {spilled}",
            pct(worst),
            pct(after_reset),
            pct(demo_worst),
            pct(median),
            ram_end - s.min_sp,
        );
        assert_eq!(spilled, 0, "{name}: a frame's work ran into the next frame");
    }
}
