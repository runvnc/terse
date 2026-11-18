//! The device calls from C (tests/devices/devices.c): results,
//! error returns, buffer rules, the 32-bit content request block, and the
//! completion of requests at the start of the next frame.

mod common;

use std::path::Path;

use terse_core::{CONTROLLERS, Completion, Controller, FrameInput, Outcome, RequestKind, pad};
use terse_core::Machine;

#[test]
fn device_calls() {
    if !common::have_tools() {
        return;
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/devices/devices.c");
    let rom = common::build(&[src], "-Os", true);
    let devices = [common::TERMINAL, common::VECTOR, common::CONTENT];
    let mut m = Machine::new(&common::program(&rom, 4, &devices));
    let mut controllers = [Controller::default(); CONTROLLERS];
    controllers[0] = Controller { connected: true, buttons: pad::A | pad::LEFT, left: (-5, 0), right: (0, 0) };
    let random = [0xAB, 2, 3, 4, 0xCD];
    let r = m.run_frame(&FrameInput { random: &random, controllers, ..Default::default() });
    assert_eq!(r.stopped, None);
    assert_eq!(r.random_used, 5);
    let requests = m.take_content_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!((requests[0].handle, &requests[0].kind), (1, &RequestKind::Get { id: [1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], offset: 3, length: 10 }));
    let RequestKind::Put { data } = &requests[1].kind else { panic!("{requests:?}") };
    assert_eq!(data, b"put from ROM");
    let put_id = *blake3::hash(data).as_bytes();
    // Completions arrive at the start of the next frame.
    let completions = [
        Completion { handle: 1, outcome: Outcome::Got { size: 20, data: (100..110).collect() } },
        Completion { handle: 2, outcome: Outcome::Put { id: put_id } },
    ];
    let r = m.run_frame(&FrameInput { completions: &completions, ..Default::default() });
    assert_eq!(r.completions_ignored, 0);
    for _ in 0..3 {
        m.run_frame(&FrameInput::default());
    }
    assert_eq!(m.exit_code(), Some(7), "{:?}", m.fault());
    let text = m.terminal().unwrap().text();
    let got: Vec<(&str, u32)> = text
        .lines()
        .filter_map(|l| l.split_once(" ").filter(|(n, _)| !n.is_empty()))
        .filter_map(|(n, v)| u32::from_str_radix(v.trim(), 16).ok().map(|v| (n, v)))
        .collect();
    let want: Vec<(&str, u32)> = vec![
        ("unknown", 0xFFFF_FFFF),
        ("absent", 0xFFFF_FFFF),
        ("ev-null", 0xFFFF_FFFE),
        ("ev-rom", 0xFFFF_FFFE),
        ("wr-bad", 0xFFFF_FFFE),
        ("vec", 0),
        ("vec-rom", 1),
        ("vec-zero", 1),
        ("style", 0 | 1 << 4 | 0 << 8),
        ("persist", 0 | 1 << 4),
        ("fill", 0 | 1 << 4),
        ("cells", 0x0102_0041 ^ 0x0304_0042),
        ("cellsbad", 0xFFFF_FFFE),
        ("palette", 2),
        ("pad0", 1 | 0x14 << 8 | 0xFB << 16),
        ("pad9", 0),
        ("random", 0xAB | 0xCD << 8),
        ("rnd-big", 0xFFFF_FFFE),
        ("size", 80 | 30 << 8),
        ("cursor", 79 | 7 << 8),
        ("get", 1),
        ("put", 2),
        ("bad", 4 << 8),
        ("blk-rom", 0),
        ("done", 1 | 1 << 8 | 1 << 16),
        ("got", 10 | 20 << 16),
        ("data", 100 | 109 << 8),
        ("put-done", 2 | 1 << 8 | (put_id[0] as u32) << 16),
    ];
    assert_eq!(got, want, "{text}");
    let t = m.terminal().unwrap();
    assert_eq!(t.palette()[255], [1, 2, 3]);
    assert_eq!(t.palette()[0], [4, 5, 6]);
    assert_eq!((t.cell(79, 29).ch, t.cell(0, 0).ch), (0x41, 0x42), "cells wrap from 2399 to 0");
    assert_eq!(terse_core::vector::dump(m.vector_list()), "disc 1 2 r 3 c ffffffff\n", "the rejected submits kept the list");
    let v = m.vector().unwrap();
    assert_eq!((v.style(), v.persistence(), v.fill()), (0, 200, false));
}
