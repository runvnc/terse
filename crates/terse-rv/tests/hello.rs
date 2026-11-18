//! programs/rv/hello/hello.terse (profile 2, assembly) shows exactly the
//! screen of the profile-1 hello (programs/hello/expected-30-frames.txt)
//! with the same input; and the image is up to date with hello.S.

mod common;

use terse_core::{CONTINUATION, Event, FrameInput};
use terse_rv::RvMachine;

const IMAGE: &[u8] = include_bytes!("../../../programs/rv/hello/hello.terse");
const EXPECTED: &str = include_str!("../../../programs/hello/expected-30-frames.txt");

fn events_for(frame: u32) -> Vec<Event> {
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

fn run(frames: u32, escape_at: Option<u32>) -> (RvMachine, Option<u8>) {
    let mut m = RvMachine::new(&terse_rv::parse_image(IMAGE).unwrap());
    let mut exit = None;
    for f in 0..frames {
        let mut events = events_for(f);
        if escape_at == Some(f) {
            events.push(Event::key_down(0x29, 0));
        }
        exit = m.run_frame(&FrameInput { events: &events, unix_time: 1_700_000_000 + f / 60, ..Default::default() }).exit;
    }
    (m, exit)
}

#[test]
fn same_screen_as_the_z80_hello() {
    let (m, exit) = run(30, None);
    assert_eq!(exit, None);
    assert_eq!(m.terminal().unwrap().text(), EXPECTED);
    assert_eq!(m.terminal().unwrap().cursor(), (5, 12));
    let t = m.terminal().unwrap();
    for (col, row) in [(17, 2), (11, 11)] {
        let cells: Vec<u16> = (col..col + 4).map(|c| t.cell(c, row).ch).collect();
        assert_eq!(cells, [0x4F60, CONTINUATION, 0x597D, CONTINUATION], "row {row}");
    }
    // The palette row: white on entries 0-6, black on the rest.
    assert_eq!((t.cell(2 + 3 * 3 + 1, 7).fg, t.cell(2 + 3 * 3 + 1, 7).bg), (15, 3));
    assert_eq!((t.cell(2 + 3 * 9 + 1, 7).fg, t.cell(2 + 3 * 9 + 1, 7).bg), (0, 9));
}

#[test]
fn exits_on_escape() {
    let (m, exit) = run(20, Some(12));
    assert_eq!(exit, Some(0));
    assert_eq!(m.frame(), 13, "no frames run after the exit");
}

#[test]
fn image_is_up_to_date() {
    if !common::have_tools() {
        return;
    }
    let rom = common::build(&[common::workspace().join("programs/rv/hello/hello.S")], "-O0", false);
    assert!(rom == terse_rv::parse_image(IMAGE).unwrap().rom, "hello.terse is out of date: run programs/rv/hello/build.sh");
}
