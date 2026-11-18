//! Device 4 through the CPU and the frame loop (SPEC 7.3, 7.5, 7.6).

mod common;

use common::*;
use terse_core::content::{BLOCK_LEN, status};
use terse_core::{Completion, DeviceReq, Event, FrameInput, Machine, Outcome, RequestKind};

const CONTENT: DeviceReq = DeviceReq { slot: 4, version: 0 };

/// Copies a get block for blob [7; 32], offset 0, length 4, data at linear
/// 1000h (CPU 9000h) to 8000h, submits it, stores the port 42h result at 8100h
/// and halts with interrupts disabled.
fn submit_get_once() -> String {
    format!(
        "
        ld hl,req_block
        ld de,8000h
        ld bc,{BLOCK_LEN}
        ldir
        xor a
        out (40h),a
        ld a,80h
        out (41h),a
        out (42h),a
        in a,(42h)
        ld (8100h),a
        halt
req_block: db 1, 0AAh
        ds 32, 7
        db 0,0,0, 4,0, 0,10h,0, 0,0,0
"
    )
}

fn got(handle: u8, data: &[u8]) -> Completion {
    Completion { handle, outcome: Outcome::Got { size: data.len() as u32, data: data.to_vec() } }
}

fn run(m: &mut Machine, completions: &[Completion]) -> terse_core::FrameResult {
    m.run_frame(&FrameInput { completions, ..Default::default() })
}

#[test]
fn absent_unless_listed_in_the_header() {
    let mut m = machine_with(&asm(&submit_get_once()), &[TERMINAL]);
    assert!(m.content().is_none());
    let r = run(&mut m, &[got(1, b"abcd")]);
    assert_eq!(m.peek(0x8100), 0xFF, "port 42h reads FFh");
    assert_eq!(m.peek(0x8001), 0xAA, "block untouched");
    assert_eq!(r.completions_ignored, 1);
    assert!(m.take_content_requests().is_empty());
}

#[test]
fn completion_never_in_the_frame_of_the_submit() {
    let mut m = machine_with(&asm(&submit_get_once()), &[CONTENT]);
    // Frame 0: the completion is applied before the CPU submits, so it matches nothing.
    let r = run(&mut m, &[got(1, b"abcd")]);
    assert_eq!(r.completions_ignored, 1);
    assert_eq!(m.peek(0x8100), 1);
    assert_eq!(m.peek(0x8001), status::PENDING);
    let reqs = m.take_content_requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].kind, RequestKind::Get { id: [7; 32], offset: 0, length: 4 });

    // Frame 1: applied at frame start, with its event queued.
    let r = run(&mut m, &[got(1, b"abcd")]);
    assert_eq!(r.completions_ignored, 0);
    assert_eq!(m.peek(0x8001), status::DONE);
    assert_eq!([m.peek(0x9000), m.peek(0x9001), m.peek(0x9002), m.peek(0x9003)], *b"abcd");
    let events: Vec<Event> = m.system().queue().copied().collect();
    assert_eq!(events, [Event::content_done(1, status::DONE)]);
}

#[test]
fn completion_events_follow_the_frames_input_events() {
    let mut m = machine_with(&asm(&submit_get_once()), &[CONTENT]);
    run(&mut m, &[]);
    let input = [Event::key_down(4, 0)];
    m.run_frame(&FrameInput { events: &input, completions: &[got(1, b"wxyz")], ..Default::default() });
    let events: Vec<Event> = m.system().queue().copied().collect();
    assert_eq!(events, [Event::key_down(4, 0), Event::content_done(1, status::DONE)]);
}

#[test]
fn content_done_dropped_when_queue_full() {
    let mut m = machine_with(&asm(&submit_get_once()), &[CONTENT]);
    run(&mut m, &[]);
    let input = vec![Event::key_down(4, 0); 64];
    let r = m.run_frame(&FrameInput { events: &input, completions: &[got(1, b"wxyz")], ..Default::default() });
    assert_eq!(r.events_dropped, 1);
    assert_eq!(m.peek(0x8001), status::DONE, "block still updated");
}

#[test]
fn put_charge_counts_against_the_budget() {
    // Put of 1000 bytes at linear 1000h.
    let src = format!(
        "
        ld hl,req_block
        ld de,8000h
        ld bc,{BLOCK_LEN}
        ldir
        xor a
        out (40h),a
        ld a,80h
        out (41h),a
        out (42h),a
        halt
req_block: db 2, 0
        ds 32, 0
        db 0,0,0, 0,0, 0,10h,0, 0E8h,03h,0
"
    );
    let mut m = machine_with(&asm(&src), &[CONTENT]);
    let r = run(&mut m, &[]);
    // ld hl(10) ld de(10) ld bc(10) ldir(44*21+16) xor(4) out(11) ld a(7) out(11) out(11+1000) halt(4)
    assert_eq!(r.tstates, 10 + 10 + 10 + 44 * 21 + 16 + 4 + 11 + 7 + 11 + 11 + 1000 + 4);
    let reqs = m.take_content_requests();
    assert_eq!(reqs[0].kind, RequestKind::Put { data: vec![0; 1000] });
}

/// Submits a get each frame (cycling through four blocks) and logs every
/// event to RAM, so the final state depends on every completion's timing.
const LOGGER: &str = "
        ld sp,0
        ld ix,0A000h
        im 1
        ei
loop    halt
        jr loop
        org 38h
        in a,(14h)
        and 3
        add a,a
        add a,a
        add a,a
        add a,a
        add a,a
        add a,a         ; block at 8000h + 64 * (frame & 3)
        ld e,a
        ld b,0
        ld c,a
        ld hl,req_block
        ld d,80h
        push bc
        ld bc,45
        ldir
        pop bc
        ld a,c
        out (40h),a
        ld a,80h
        out (41h),a
        out (42h),a
        in a,(42h)
        ld (ix+0),a
        inc ix
events  in a,(10h)
        or a
        jr z,done
        ld (ix+0),a
        in a,(11h)
        ld (ix+1),a
        in a,(12h)
        ld (ix+2),a
        inc ix
        inc ix
        inc ix
        out (10h),a
        jr events
done    ei
        reti
req_block: db 1, 0
        ds 32, 9
        db 0,0,0, 8,0, 0,10h,0, 0,0,0
";

fn logger_run(schedule: &[(u32, Completion)]) -> Machine {
    let mut m = machine_with(&asm(LOGGER), &[CONTENT]);
    for f in 0..12 {
        let completions: Vec<Completion> =
            schedule.iter().filter(|(at, _)| *at == f).map(|(_, c)| c.clone()).collect();
        m.run_frame(&FrameInput { completions: &completions, ..Default::default() });
        m.take_content_requests();
    }
    m
}

#[test]
fn determinism_with_recorded_completions() {
    let schedule = vec![
        (2, got(1, b"12345678")),
        (2, Completion { handle: 2, outcome: Outcome::NotFound }),
        (5, Completion { handle: 3, outcome: Outcome::Failed }),
        (5, got(1, b"abc")),
        (9, got(4, b"zz")),
    ];
    let (a, b) = (logger_run(&schedule), logger_run(&schedule));
    assert_eq!(a.registers(), b.registers());
    assert_eq!(a.ram(), b.ram());
    assert_eq!(a.content(), b.content());
    assert!(a.ram()[0x2000..0x2040].iter().any(|&x| x == 5), "content-done events were logged");
    // Different timing, different state.
    let mut later = schedule.clone();
    later[0].0 = 3;
    assert_ne!(logger_run(&later).ram(), a.ram());
}
