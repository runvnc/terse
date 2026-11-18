//! Golden test: programs/hello/hello.terse with fixed inputs.
//! Set UPDATE_GOLDEN=1 to rewrite the expected file.

use terse_core::{CONTINUATION, Event, FrameInput, Image, Machine};

const IMAGE: &[u8] = include_bytes!("../../../programs/hello/hello.terse");
const EXPECTED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../programs/hello/expected-30-frames.txt");

const KEY_ENTER: u8 = 0x28;
const KEY_ESCAPE: u8 = 0x29;
const KEY_BACKSPACE: u8 = 0x2A;

fn events_for(frame: u32) -> Vec<Event> {
    let text = |s: &str| s.chars().map(Event::text).collect::<Vec<_>>();
    match frame {
        5 => text("Hi wörld "),
        6 => text("你好"),
        7 => vec![Event::key_down(KEY_ENTER, 0), Event::key_up(KEY_ENTER, 0)],
        8 => text("😀x"), // above U+FFFF: shown as U+FFFD
        9 => vec![Event::key_down(KEY_BACKSPACE, 0)],
        10 => text("ok"),
        _ => vec![],
    }
}

fn run(frames: u32, escape_at: Option<u32>) -> (Machine, Option<u8>) {
    let mut m = Machine::new(&Image::parse(IMAGE).unwrap());
    let mut exit = None;
    for f in 0..frames {
        let mut events = events_for(f);
        if escape_at == Some(f) {
            events.push(Event::key_down(KEY_ESCAPE, 0));
        }
        let input = FrameInput { events: &events, unix_time: 1_700_000_000 + f / 60, random: &[], completions: &[] };
        exit = m.run_frame(&input).exit;
    }
    (m, exit)
}

#[test]
fn hello_screen_after_30_frames() {
    let (m, exit) = run(30, None);
    assert_eq!(exit, None);
    let got = m.terminal().unwrap().text();
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(EXPECTED, &got).unwrap();
    }
    let want = std::fs::read_to_string(EXPECTED).expect("expected file (run with UPDATE_GOLDEN=1)");
    assert_eq!(got, want);
    let (col, row) = m.terminal().unwrap().cursor();
    assert_eq!((col, row), (5, 12));
}

/// 你好 takes four cells, both in the static text and in the echo (SPEC 6.1).
#[test]
fn hello_wide_characters_take_two_cells() {
    let (m, _) = run(30, None);
    let t = m.terminal().unwrap();
    for (col, row) in [(17, 2), (11, 11)] {
        let cells: Vec<u16> = (col..col + 4).map(|c| t.cell(c, row).ch).collect();
        assert_eq!(cells, [0x4F60, CONTINUATION, 0x597D, CONTINUATION], "row {row}");
    }
}

#[test]
fn hello_exits_on_escape() {
    let (m, exit) = run(20, Some(12));
    assert_eq!(exit, Some(0));
    assert_eq!(m.frame(), 13, "no frames run after the exit");
}
