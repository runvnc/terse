//! The state hash (STATE.md, SPEC 9): stable across identical runs, sensitive
//! to each kind of state, and golden for programs/hello.

mod common;

use common::*;
use terse_core::{Controller, DeviceReq, Event, FrameInput, Image, Machine, STATE_DOMAIN};

const VECTOR: DeviceReq = DeviceReq { slot: 3, version: 0 };
const CONTENT: DeviceReq = DeviceReq { slot: 4, version: 0 };
const ALL: [DeviceReq; 3] = [TERMINAL, VECTOR, CONTENT];

fn hex(h: &[u8; 32]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// The hash after one frame of `src` with `input`.
fn hash_after(src: &str, input: &FrameInput) -> [u8; 32] {
    let mut m = machine_with(&asm(src), &ALL);
    m.run_frame(input);
    m.state_hash()
}

/// `template` with `{}` replaced by `a` and by `b` must hash differently;
/// the same source twice must hash the same.
fn differs(what: &str, template: &str, a: &str, b: &str) {
    let (sa, sb) = (template.replace("{}", a), template.replace("{}", b));
    let input = FrameInput::default();
    assert_eq!(hash_after(&sa, &input), hash_after(&sa, &input), "{what}: not stable");
    // Same code size, so only the state named differs.
    assert_eq!(asm(&sa).len(), asm(&sb).len(), "{what}: variants must assemble to the same size");
    assert_ne!(hash_after(&sa, &input), hash_after(&sb, &input), "{what}: hash ignores it");
}

#[test]
fn layout_starts_with_domain_and_image_id() {
    let m = machine_with(&asm("halt\n"), &ALL);
    let bytes = m.state_bytes();
    assert_eq!(&bytes[..13], STATE_DOMAIN);
    assert_eq!(&bytes[13..45], m.image_id());
    assert_eq!(m.state_hash(), *blake3::hash(&bytes).as_bytes());
    // 13 + 32, CPU 26 + 9, frame loop 4 + 2 + 4 + 1, memory 8 + 4 + RAM,
    // system 4 + 1 + 32 + 2, terminal 1 + 9600 + 6 + 2 + 768 + 2 + 4,
    // vector 1 + 4 + 2 + 1 + 2 + 4, content 1 + 2 + 1 + 8 * 51.
    let ram = 4 * 8192;
    let want = 45 + 35 + 11 + 12 + ram + 39 + 10_383 + 14 + 412;
    assert_eq!(bytes.len(), want);
    // Without devices 2-4 each section is one zero byte.
    let bare = machine_with(&asm("halt\n"), &[]).state_bytes();
    assert_eq!(bare.len(), want - 10_382 - 13 - 411);
}

#[test]
fn identical_runs_hash_identically() {
    let image = Image::parse(include_bytes!("../../../programs/hello/hello.terse")).unwrap();
    let run = || {
        let mut m = Machine::new(&image);
        (0..30)
            .map(|f| {
                let events = if f == 5 { vec![Event::text('x')] } else { vec![] };
                m.run_frame(&FrameInput { events: &events, ..Default::default() });
                m.state_hash()
            })
            .collect::<Vec<_>>()
    };
    let (a, b) = (run(), run());
    assert_eq!(a, b);
    // Every frame of hello changes something (at least the frame counter).
    assert!(a.windows(2).all(|w| w[0] != w[1]));
}

/// programs/hello for 30 frames with no input, as `terse run --frames 30 --hash`.
#[test]
fn hello_golden_hash() {
    let mut m = Machine::new(&Image::parse(include_bytes!("../../../programs/hello/hello.terse")).unwrap());
    // At reset (STATE.md test vectors).
    assert_eq!(m.state_bytes().len(), 43_295);
    assert_eq!(hex(&m.state_hash()), "013759c33d61daa0768a77f6bcb428a88e2273a7e412d35d09496944b3338b53");
    let mut hashes = Vec::new();
    for _ in 0..30 {
        m.run_frame(&FrameInput::default());
        if m.frame() % 10 == 0 {
            hashes.push(hex(&m.state_hash()));
        }
    }
    assert_eq!(
        hashes,
        [
            "c12ad8c0d62f78f5514c6cd812336c16ceb8bb56655c62e2d48ce96d24e94d85",
            "b5f641feb69644c537f7ab0838955eb47f11577750c3dec78e1e3a99ee672915",
            "7d79f4c1e255bf16c58ecba580f2e4d38ad27f324fde57883b9689ca5a4b538c",
        ]
    );
}

#[test]
fn cpu_state() {
    differs("main register", "ld b,{}\n halt\n", "1", "2");
    differs("alternate register", "exx\n ld b,{}\n exx\n halt\n", "1", "2");
    differs("alternate AF", "ex af,af'\n ld a,{}\n ex af,af'\n halt\n", "1", "2");
    differs("IX", "ld ix,{}\n halt\n", "1", "2");
    differs("SP", "ld sp,{}\n halt\n", "1", "2");
    differs("I", "ld a,{}\n ld i,a\n xor a\n halt\n", "1", "2");
    differs("R", "ld a,{}\n ld r,a\n xor a\n halt\n", "1", "2");
    differs("interrupt mode", "im {}\n halt\n", "1", "2");
    differs("IFF", "{}\n halt\n", "di", "ei");
    // MEMPTR: both loads read zero; only the address differs.
    differs("MEMPTR", "ld a,({})\n halt\n", "9000h", "9002h");
    // Halted, or stopped by the budget in a loop at the same PC.
    let halted = hash_after("halt\n", &FrameInput::default());
    let mut m = machine_with(&asm("x: jr x\n"), &ALL);
    m.run_frame(&FrameInput::default());
    assert_ne!(halted, m.state_hash());
}

#[test]
fn frame_loop_state() {
    differs("exit code", "ld a,{}\n out (1Fh),a\n halt\n", "1", "2");
    // A negative remainder (11 T-states per RST: 300003 = -3) against a
    // frame that halts at the same PC.
    let mut a = machine_with(&asm("rst 38h\n org 38h\n rst 38h\n"), &ALL);
    a.run_frame(&FrameInput::default());
    assert_eq!(a.budget(), -3);
    let h1 = a.state_hash();
    a.run_frame(&FrameInput::default());
    assert_eq!(a.budget(), -6);
    assert_ne!(h1, a.state_hash());
    // Interrupt request still set (interrupts disabled) vs accepted.
    differs("interrupt request", "im 1\n {}\n halt\n org 38h\n halt\n", "di", "ei");
}

#[test]
fn memory_state() {
    differs("RAM", "ld a,1\n ld ({}),a\n halt\n", "9000h", "9001h");
    differs("slot register", "ld a,{}\n out (5),a\n xor a\n halt\n", "85h", "86h");
    // The same address through slot 4 showing different pages.
    differs("RAM page", "ld a,{}\n out (4),a\n ld a,1\n ld (8000h),a\n ld a,80h\n out (4),a\n halt\n", "82h", "83h");
}

#[test]
fn system_state() {
    let src = "halt\n";
    let none = hash_after(src, &FrameInput::default());
    let events = [Event::key_down(4, 0)];
    let with_event = hash_after(src, &FrameInput { events: &events, ..Default::default() });
    let other = [Event::key_down(5, 0)];
    let with_other = hash_after(src, &FrameInput { events: &other, ..Default::default() });
    assert!(none != with_event && with_event != with_other);
    assert_ne!(none, hash_after(src, &FrameInput { unix_time: 1, ..Default::default() }), "time latch");
    let mut pads = [Controller::default(); 4];
    pads[2].connected = true;
    assert_ne!(none, hash_after(src, &FrameInput { controllers: pads, ..Default::default() }), "controllers");
    // Random bytes are consumed within the frame and are not state.
    assert_eq!(none, hash_after(src, &FrameInput { random: &[1, 2, 3], ..Default::default() }));
    differs("controller selection", "ld a,{}\n out (1Dh),a\n xor a\n halt\n", "1", "2");
    differs("controller read position", "{}\n halt\n", "in a,(1Eh)\n nop\n nop", "in a,(1Eh)\n in a,(1Eh)");
}

#[test]
fn terminal_state() {
    differs("cell", "ld a,{}\n out (20h),a\n xor a\n halt\n", "'a'", "'b'");
    differs("cursor", "ld a,{}\n out (21h),a\n xor a\n halt\n", "1", "2");
    differs("colour", "ld a,{}\n out (23h),a\n xor a\n halt\n", "1", "2");
    differs("cursor shown", "ld a,{}\n out (25h),a\n xor a\n halt\n", "5", "6");
    differs("cell pointer", "ld a,{}\n out (27h),a\n xor a\n halt\n", "1", "2");
    differs("palette", "ld a,{}\n out (2Ah),a\n xor a\n halt\n", "1", "2");
    differs("palette index", "ld a,{}\n out (29h),a\n xor a\n halt\n", "1", "2");
    // The second palette write: same colours, different component.
    differs("palette component", "ld a,0\n {}\n xor a\n halt\n", "out (2Ah),a\n nop\n nop", "out (2Ah),a\n out (29h),a");
    // An incomplete UTF-8 sequence: the cells are the same, the decoder is not.
    differs("put-character decoder", "ld a,{}\n out (20h),a\n xor a\n halt\n", "0C3h", "0C4h");
    // Pending wrap: column 79 either way.
    differs(
        "pending wrap",
        "ld a,79\n out (21h),a\n ld a,'x'\n out (20h),a\n ld a,79\n {}\n xor a\n halt\n",
        "nop\n nop",
        "out (21h),a",
    );
}

#[test]
fn vector_state() {
    differs("vector address", "ld a,{}\n out (31h),a\n xor a\n halt\n", "1", "2");
    differs("vector length", "ld a,{}\n out (33h),a\n xor a\n halt\n", "1", "2");
    differs("vector result", "ld a,{}\n out (35h),a\n xor a\n halt\n", "2", "3");
    differs("vector style", "ld a,{}\n out (36h),a\n xor a\n halt\n", "0", "1");
    differs("vector persistence", "ld a,{}\n out (37h),a\n xor a\n halt\n", "1", "2");
    // Lists: one byte of RAM, submitted, then RAM restored.
    differs(
        "vector list",
        "ld a,{}\n ld (8000h),a\n ld a,1\n out (33h),a\n out (35h),a\n xor a\n ld (8000h),a\n halt\n",
        "7",
        "8",
    );
}

#[test]
fn content_state() {
    differs("block address", "ld a,{}\n out (40h),a\n xor a\n halt\n", "1", "2");
    // A get request pending: the block is written, submitted, then erased
    // so RAM is the same; only the pending request differs.
    let get = "ld hl,8100h\n ld (hl),1\n ld hl,8102h\n ld (hl),{}\n ld hl,8125h\n ld (hl),1\n ld hl,8128h\n ld (hl),10h\n \
               ld a,0\n out (40h),a\n ld a,81h\n out (41h),a\n out (42h),a\n \
               ld hl,8100h\n ld de,8101h\n ld bc,44\n ld (hl),0\n ldir\n halt\n";
    differs("pending get id", get, "1", "2");
}
