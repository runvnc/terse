//! programs/hello/hello.terse (assembly): the golden screen after 30 frames
//! of typed input (programs/hello/expected-30-frames.txt), Esc exits, and
//! the image is up to date with hello.S.

mod common;

use terse_core::{CONTINUATION, Event, FrameInput, Image, Machine};

const IMAGE: &[u8] = include_bytes!("../../../programs/hello/hello.terse");

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

fn run(frames: u32, escape_at: Option<u32>) -> (Machine, Option<u8>, Vec<u64>) {
    let mut m = Machine::new(&Image::parse(IMAGE).unwrap());
    let mut exit = None;
    let mut used = Vec::new();
    for f in 0..frames {
        let mut events = events_for(f);
        if escape_at == Some(f) {
            events.push(Event::key_down(0x29, 0));
        }
        let r = m.run_frame(&FrameInput { events: &events, unix_time: 1_700_000_000 + f / 60, ..Default::default() });
        assert!(m.fault().is_none(), "{:?}", m.fault());
        used.push(r.used);
        exit = r.exit();
    }
    (m, exit, used)
}

#[test]
fn screen_after_30_frames() {
    let (m, exit, used) = run(30, None);
    assert_eq!(exit, None);
    let t = m.terminal().unwrap();
    common::golden("hello/expected-30-frames.txt", &t.text());
    assert_eq!(t.cursor(), (5, 12));
    for (col, row) in [(17, 2), (11, 11)] {
        let cells: Vec<u16> = (col..col + 4).map(|c| t.cell(c, row).ch).collect();
        assert_eq!(cells, [0x4F60, CONTINUATION, 0x597D, CONTINUATION], "row {row}");
    }
    // The palette row: white on entries 0-6, black on the rest.
    assert_eq!((t.cell(2 + 3 * 3 + 1, 7).fg, t.cell(2 + 3 * 3 + 1, 7).bg), (15, 3));
    assert_eq!((t.cell(2 + 3 * 9 + 1, 7).fg, t.cell(2 + 3 * 9 + 1, 7).bg), (0, 9));
    eprintln!("hello: {}", common::FrameStats::new(&used).describe());
}

#[test]
fn exits_on_escape() {
    let (m, exit, _) = run(20, Some(12));
    assert_eq!(exit, Some(0));
    assert_eq!(m.frame(), 13, "no frames run after the exit");
}

#[test]
fn image_is_up_to_date() {
    common::check_up_to_date(IMAGE, &["hello/hello.S"], false, "hello", 1, &[(2, 0), (10, 0x80)]);
}
