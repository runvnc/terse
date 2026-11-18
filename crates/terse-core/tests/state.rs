//! The state serialization and hash (STATE.md): the layout at the offsets
//! the document gives, the stopped states, sensitivity to each part, and the
//! test vectors for programs/hello.

mod common;

use common::{assemble, frame};
use terse_core::{
    Completion, Controller, DeviceReq, Event, FrameInput, Image, Machine, Outcome, RequestKind, STATE_DOMAIN, Stopped,
    charge,
};

const ALL: [DeviceReq; 4] = [common::TERMINAL, common::VECTOR, common::CONTENT, common::PAGE];

fn u32_at(s: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(s[at..at + 4].try_into().unwrap())
}

fn hex(h: &[u8; 32]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn layout_at_reset() {
    let image = common::program(&[0x01, 0x00], 2, &ALL);
    let m = Machine::new(&image);
    let s = m.state();
    let l = 2 * 8192;
    assert_eq!(&s[..13], STATE_DOMAIN);
    assert_eq!(&s[..13], b"terse-state-1");
    assert_eq!(&s[13..45], &image.id);
    assert_eq!(u32_at(&s, 45), 0x0001_0000, "pc");
    assert_eq!(u32_at(&s, 49 + 4), 0x0010_0000 + l as u32, "x2 = sp is the second register");
    assert_eq!(u32_at(&s, 173), 0, "frame counter");
    assert_eq!(&s[177..196], &[0; 19], "running, no fault, budget 0");
    assert_eq!(u32_at(&s, 196), l as u32);
    let sys = 200 + l;
    assert_eq!(s[sys + 4], 0, "no events");
    let t = sys + 37;
    assert_eq!(s[t], 1, "terminal present");
    assert_eq!(&s[t + 1..t + 5], &[0x20, 0, 7, 0], "cell 0");
    assert_eq!(&s[t + 9601..t + 9607], &[0, 0, 7, 0, 1, 0], "cursor, colours, shown, wrap");
    assert_eq!(&s[t + 9607..t + 9610], &[0, 0, 0], "palette entry 0");
    let v = t + 10_379;
    assert_eq!(&s[v..v + 8], &[1, 0, 160, 0, 0, 0, 0, 0], "vector: present, clean, 160, fill, empty");
    let c = v + 8;
    assert_eq!(s[c], 1);
    let p = c + 425;
    assert_eq!(
        &s[p..],
        &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0, 0, 0, 0],
        "page: present, region 0, scroll 0, background FF000000h, no document"
    );
    assert_eq!(s.len(), 45 + 128 + 23 + 4 + l + 37 + 10_379 + 8 + 425 + 21);

    // Without devices each section is one flag.
    let bare = Machine::new(&common::program(&[0x01, 0x00], 1, &[]));
    assert_eq!(bare.state().len(), 45 + 128 + 23 + 4 + 8192 + 37 + 4);
}

#[test]
fn stopped_states_and_budget() {
    if !common::have_tools() {
        return;
    }
    let mut m = common::machine(&assemble("    li a0, 0x1234\n    li a7, 0x106\n    ecall\n"), &[]);
    frame(&mut m);
    assert_eq!(m.stopped(), Some(Stopped::Exit(0x34)));
    let s = m.state();
    assert_eq!(&s[177..180], &[1, 0x34, 0]);
    assert_eq!(u32_at(&s, 173), 1);

    let mut m = common::machine(&assemble("    li a1, 8\n    sw a0, 0(a1)\n"), &[]);
    frame(&mut m);
    let s = m.state();
    assert_eq!(&s[177..180], &[2, 0, 5], "faulted: store");
    assert_eq!((u32_at(&s, 180), u32_at(&s, 184), u32_at(&s, 188)), (0, 0x0001_0002, 8));
    assert_eq!(u32_at(&s, 45), 0x0001_0002, "pc at the faulting instruction");

    let mut m = common::machine(&[0x00, 0x00], &[]);
    frame(&mut m);
    assert_eq!(&m.state()[177..184], &[2, 0, 1, 0, 0, 0, 0], "illegal 0000h");

    // A negative budget carried into the next frame.
    let src = "    li a7, 0x100\n    ecall\n    lui t0, 15\n    addi t0, t0, 460\n1:  addi t0, t0, -1\n    bnez t0, 1b\n\
               li a0, 0\n    li a1, 0x100000\n    li a2, 2400\n    li a7, 0x207\n    ecall\n2:  j 2b\n";
    let mut m = common::machine(&assemble(src), &[common::TERMINAL]);
    frame(&mut m);
    frame(&mut m);
    assert!(m.budget() < 0);
    assert_eq!(i32::from_le_bytes(m.state()[192..196].try_into().unwrap()) as i64, m.budget());
    let _ = charge::CALL;
}

/// The hash after one frame of `src` with `input`.
fn hash_after(src: &str, input: &FrameInput) -> [u8; 32] {
    let mut m = Machine::new(&common::program(&assemble(src), 2, &ALL));
    m.run_frame(input);
    m.state_hash()
}

#[test]
fn sensitive_to_each_part() {
    if !common::have_tools() {
        return;
    }
    let base = "    li a7, 0x100\n    ecall\n";
    let none = FrameInput::default();
    let h = hash_after(base, &none);
    assert_eq!(h, hash_after(base, &none), "identical runs hash identically");
    let differs = |src: &str, input: &FrameInput, what: &str| assert_ne!(hash_after(src, input), h, "{what}");
    differs(&format!("    li s0, 1\n{base}"), &none, "a register");
    differs(&format!("    li t0, 0x100000\n    sw t0, 8(t0)\n{base}"), &none, "RAM");
    differs(base, &FrameInput { unix_time: 1, ..Default::default() }, "time latch");
    differs(base, &FrameInput { events: &[Event::key_down(4, 0)], ..Default::default() }, "event queue");
    let mut pads = [Controller::default(); 4];
    pads[2].connected = true;
    differs(base, &FrameInput { controllers: pads, ..Default::default() }, "controllers");
    // Random bytes are not state unless the program keeps them.
    assert_eq!(hash_after(base, &FrameInput { random: &[1, 2, 3], ..Default::default() }), h);
    differs(&format!("    li a0, 0x100000\n    li a1, 1\n    li a7, 0x200\n    ecall\n{base}"), &none, "terminal");
    differs(&format!("    li a0, 1\n    li a7, 0x302\n    ecall\n{base}"), &none, "style");
    differs(&format!("    li a0, 1\n    li a7, 0x304\n    ecall\n{base}"), &none, "fill mode");
    differs(&format!("    li a0, 9\n    li a7, 0x303\n    ecall\n{base}"), &none, "persistence");
    differs(&format!("    li a0, 0x10000\n    li a1, 2\n    li a7, 0x700\n    ecall\n{base}"), &none, "page document");
    differs(&format!("    li a2, 5\n    li a7, 0x702\n    ecall\n{base}"), &none, "page region");
    differs(&format!("    li a0, -1\n    li a7, 0x703\n    ecall\n{base}"), &none, "page scroll");
    differs(&format!("    li a0, 1\n    li a7, 0x704\n    ecall\n{base}"), &none, "page background");
    // A content get pending (op 1, length 1, addr in RAM, block at RAM).
    let get = "    li t0, 0x100000\n    li t1, 1\n    sb t1, 0(t0)\n    sw t1, 40(t0)\n    li t1, 0x100100\n    \
               sw t1, 44(t0)\n    mv a0, t0\n    li a7, 0x400\n    ecall\n";
    differs(&format!("{get}{base}"), &none, "content request");
    // Completing it changes the block, the data and the queue, and frees the slot.
    let mut m = Machine::new(&common::program(&assemble(&format!("{get}{base}1:  {base}    j 1b\n")), 2, &ALL));
    m.run_frame(&none);
    let pending = m.state();
    let done = [Completion { handle: 1, outcome: Outcome::Got { size: 1, data: vec![9] } }];
    m.run_frame(&FrameInput { completions: &done, ..Default::default() });
    let s = m.state();
    let content = |s: &[u8]| s[s.len() - 21 - 425..s.len() - 21].to_vec();
    assert_ne!(content(&pending), content(&s));
    assert!(content(&s)[1..].iter().all(|&b| b == 0), "slot 1 free again");
}

#[test]
fn hello_test_vectors() {
    let path = common::workspace().join("programs/hello/hello.terse");
    let image = Image::parse(&std::fs::read(path).unwrap()).unwrap();
    let mut m = Machine::new(&image);
    let reset = m.state();
    let mut hashes = vec![(reset.len(), hex(&m.state_hash()))];
    for n in 1..=30 {
        m.run_frame(&FrameInput::default());
        if n == 1 || n == 30 {
            hashes.push((0, hex(&m.state_hash())));
        }
    }
    // STATE.md section 3 quotes these.
    let text = std::fs::read_to_string(common::workspace().join("STATE.md")).unwrap();
    for (len, h) in &hashes {
        println!("{len} {h}");
        assert!(text.contains(h.as_str()), "STATE.md does not quote {h}");
    }
    assert!(text.contains(&format!("{} bytes", group(hashes[0].0))), "STATE.md does not give the length {}", hashes[0].0);
}

/// 43295 -> "43,295".
fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// SPEC 9.5: subscribe, publish and identity requests are inputs like any
/// other completion: the same inputs replay to the same hashes, the pending
/// records are in the state, and a different answer gives a different state.
#[test]
fn feed_operations_replay() {
    if !common::have_tools() {
        return;
    }
    let base = "    li a7, 0x100\n    ecall\n";
    // Subscribe (op 5, id byte 0 = s1) at 100000h; publish (op 7, title
    // "hi" at 100100h) at 100040h; identity (op 8) at 100080h.
    let src = |key: u8| {
        format!(
            "    li t0, 0x100000\n    li t1, 5\n    sb t1, 0(t0)\n    li t1, {key}\n    sb t1, 4(t0)\n    mv a0, t0\n    \
             li a7, 0x400\n    ecall\n\
             li t2, 0x100100\n    li t1, 0x6968\n    sh t1, 0(t2)\n    li t0, 0x100040\n    li t1, 7\n    sb t1, 0(t0)\n    \
             li t1, 2\n    sw t1, 40(t0)\n    sw t2, 44(t0)\n    mv a0, t0\n    li a7, 0x400\n    ecall\n\
             li t0, 0x100080\n    li t1, 8\n    sb t1, 0(t0)\n    mv a0, t0\n    li a7, 0x400\n    ecall\n\
             1:  {base}    j 1b\n"
        )
    };
    let answers = |subscribe: Outcome| {
        vec![
            Completion { handle: 1, outcome: subscribe },
            Completion { handle: 2, outcome: Outcome::Published { seq: 3 } },
            Completion { handle: 3, outcome: Outcome::Identity { key: [0x5A; 32] } },
        ]
    };
    let run = |key: u8, subscribe: Outcome| -> Vec<[u8; 32]> {
        let want = if subscribe == Outcome::Refused { 5 } else { 1 };
        let mut m = Machine::new(&common::program(&assemble(&src(key)), 2, &ALL));
        let mut hashes = Vec::new();
        m.run_frame(&FrameInput::default());
        let requests = m.take_content_requests();
        assert_eq!(requests.len(), 3, "{requests:?}");
        assert_eq!(requests[1].kind, terse_core::RequestKind::Publish { blob: [0; 32], title: b"hi".to_vec() });
        hashes.push(m.state_hash());
        let done = answers(subscribe);
        let r = m.run_frame(&FrameInput { completions: &done, ..Default::default() });
        assert_eq!(r.completions_ignored, 0);
        hashes.push(m.state_hash());
        m.run_frame(&FrameInput::default());
        hashes.push(m.state_hash());
        let ram = m.ram();
        assert_eq!((ram[1], ram[0x41], ram[0x81]), (want, 1, 1), "statuses");
        assert_eq!(&ram[0x40 + 48..0x40 + 52], &[3, 0, 0, 0], "size is the seq");
        assert_eq!(&ram[0x80 + 4..0x80 + 36], &[0x5A; 32], "id is the key");
        hashes
    };
    let a = run(7, Outcome::Done);
    assert_eq!(a, run(7, Outcome::Done), "the same inputs replay exactly");
    let other_key = run(8, Outcome::Done);
    assert_ne!(a[0], other_key[0], "the pending subscribe's feed key is state");
    let refused = run(7, Outcome::Refused);
    assert_eq!(a[0], refused[0]);
    assert_ne!(a[1], refused[1], "refused and done differ");
}

/// SPEC 9.5 and 12: find, set profile and set nickname copy their query or
/// name at submit and are charged for it (1 per 4 bytes); names and find are
/// records requests; the same inputs replay to the same hashes.
#[test]
fn names_find_and_profile_operations_replay() {
    if !common::have_tools() {
        return;
    }
    let base = "    li a7, 0x100\n    ecall\n";
    let submit = "    mv a0, t0\n    li a7, 0x400\n    ecall\n";
    // "hihihihi" at 100100h; a3 = n, the query and name length. Find (op
    // 0Ah, scope 2, 64-byte buffer) at 100000h; set profile (op 0Bh,
    // listed) at 100040h; set nickname (op 0Ch, key byte 0 = 33h) at
    // 100080h; names (op 09h, 64 bytes at 100200h) at 1000C0h.
    let src = |n: u8| {
        format!(
            "    li t2, 0x100100\n    li t1, 0x69686968\n    sw t1, 0(t2)\n    sw t1, 4(t2)\n    li a3, {n}\n\
             li t0, 0x100000\n    li t1, 10\n    sb t1, 0(t0)\n    li t1, 2\n    sw t1, 36(t0)\n    li t1, 64\n    \
             sw t1, 40(t0)\n    sw t2, 44(t0)\n    sw a3, 48(t0)\n{submit}\
             li t0, 0x100040\n    li t1, 11\n    sb t1, 0(t0)\n    li t1, 1\n    sw t1, 36(t0)\n    sw a3, 40(t0)\n    \
             sw t2, 44(t0)\n{submit}\
             li t0, 0x100080\n    li t1, 12\n    sb t1, 0(t0)\n    li t1, 0x33\n    sb t1, 4(t0)\n    sw a3, 40(t0)\n    \
             sw t2, 44(t0)\n{submit}\
             li t0, 0x1000C0\n    li t1, 9\n    sb t1, 0(t0)\n    li t1, 64\n    sw t1, 40(t0)\n    li t1, 0x100200\n    \
             sw t1, 44(t0)\n{submit}\
             1:  {base}    j 1b\n"
        )
    };
    let mut record = vec![35, 2];
    record.extend([0x44; 32]);
    record.extend(b"hi");
    let answers = [
        Completion { handle: 1, outcome: Outcome::Records { total: 1, data: Vec::new() } },
        Completion { handle: 2, outcome: Outcome::Published { seq: 4 } },
        Completion { handle: 3, outcome: Outcome::Done },
        Completion { handle: 4, outcome: Outcome::Records { total: 1, data: record.clone() } },
    ];
    let mut key = [0; 32];
    key[0] = 0x33;
    let run = |n: u8| -> (u64, Vec<[u8; 32]>) {
        let mut m = Machine::new(&common::program(&assemble(&src(n)), 2, &ALL));
        let used = m.run_frame(&FrameInput::default()).used;
        let name = b"hihihihi"[..n as usize].to_vec();
        assert_eq!(
            m.take_content_requests().into_iter().map(|r| r.kind).collect::<Vec<_>>(),
            [
                RequestKind::Find { scope: 2, query: name.clone(), length: 64 },
                RequestKind::SetProfile { name: name.clone(), listed: true },
                RequestKind::SetNickname { feed: key, name },
                RequestKind::Names { offset: 0, length: 64 },
            ]
        );
        let mut hashes = vec![m.state_hash()];
        let r = m.run_frame(&FrameInput { completions: &answers, ..Default::default() });
        assert_eq!(r.completions_ignored, 0);
        hashes.push(m.state_hash());
        let ram = m.ram();
        assert_eq!((ram[1], ram[0x41], ram[0x81], ram[0xC1]), (1, 1, 1, 1), "statuses");
        assert_eq!(&ram[0x40 + 48..0x40 + 52], &[4, 0, 0, 0], "size is the seq");
        assert_eq!(&ram[0x200..0x200 + 36], &record[..]);
        assert_eq!(&ram[0xC0 + 40..0xC0 + 52], &[36, 0, 0, 0, 0, 0x02, 0x10, 0, 1, 0, 0, 0], "length, addr, size");
        (used, hashes)
    };
    let (used2, a) = run(2);
    assert_eq!(a, run(2).1, "the same inputs replay exactly");
    let (used6, b) = run(6);
    assert_eq!(used6 - used2, 3, "three copies of 6 bytes cost 2 units each, of 2 bytes 1");
    assert_ne!(a[0], b[0], "the name lengths are state");
}
