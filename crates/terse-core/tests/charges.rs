//! Host cost per budget unit of each kind of call, against plain ROM
//! instructions (SPEC 13): a frame spent entirely in one call type should
//! cost the host about what a frame of instructions costs. `measure` prints
//! the table (run in release; it is ignored by default):
//!
//!     cargo test --release -p terse-core --test charges -- --ignored --nocapture --test-threads 1
//!
//! `all_loops_run` checks the guest loops themselves in every build.

mod common;

use std::time::Instant;

use terse_core::{Completion, FrameInput, MAX_RANDOM_PER_FRAME, Machine, Outcome, RequestKind};

/// Fills RAM 0x100000.. with `n` bytes of `fill` (t0 = address, t1 = end).
fn fill_ram(n: u32, fill: u8) -> String {
    format!(
        "    li t0, 0x100000\n    li t1, {}\n    li t2, {fill}\n0:  sb t2, 0(t0)\n    addi t0, t0, 1\n    bltu t0, t1, 0b\n",
        0x100000 + n
    )
}

/// A loop making one call forever: `setup` runs once, then `args` and the
/// ecall repeat.
fn call_loop(setup: &str, args: &str, n: u32) -> String {
    format!("{setup}1:\n{args}    li a7, {n}\n    ecall\n    j 1b\n")
}

struct Case {
    name: &'static str,
    src: String,
    ram_pages: u8,
}

fn cases() -> Vec<Case> {
    let c = |name, src: String, ram_pages| Case { name, src, ram_pages };
    // Plain instructions: ALU, loads, stores and a branch, as compiled C does.
    let plain = "    li s0, 0x100000\n    li s1, 0\n1:  lw t0, 0(s0)\n    addi t0, t0, 3\n    xor t1, t0, s1\n    \
                 slli t2, t1, 2\n    sw t2, 4(s0)\n    add s1, s1, t2\n    andi t3, s1, 255\n    sltu t4, t3, t0\n    \
                 add s1, s1, t4\n    addi s1, s1, 1\n    bnez s0, 1b\n";
    // A table of a wide character and a narrow one, as UTF-8.
    let cjk = "你a".repeat(40);
    let block = |op: u32, size: u32| {
        // A content block at 0x100000: op, addr 0x101000, size.
        format!(
            "    li t0, 0x100000\n    li t1, {op}\n    sb t1, 0(t0)\n    li t1, 0x101000\n    sw t1, 44(t0)\n    \
             li t1, {size}\n    sw t1, 48(t0)\n    li t1, 16384\n    sw t1, 40(t0)\n"
        )
    };
    vec![
        c("plain instructions (ROM)", plain.to_string(), 1),
        c("frame_count (call only)", call_loop("", "", 0x102), 1),
        c("set_cursor (call only)", call_loop("", "    li a0, 5\n    li a1, 6\n", 0x202), 1),
        c("next_event (empty queue)", call_loop("", "    li a0, 0x100000\n", 0x101), 1),
        c("controller", call_loop("", "    li a0, 0\n    li a1, 0x100000\n", 0x105), 1),
        c("random 256", call_loop("", "    li a0, 0x100000\n    li a1, 256\n", 0x104), 1),
        c("random 4", call_loop("", "    li a0, 0x100000\n    li a1, 4\n", 0x104), 1),
        c(
            "write 80 ASCII",
            call_loop(&format!("{}    j 1f\nascii: .ascii \"{}\"\n    .balign 2\n", "", "x".repeat(80)), "    la a0, ascii\n    li a1, 80\n", 0x200),
            1,
        ),
        c(
            "write 1 ASCII",
            call_loop("    j 1f\nascii: .ascii \"xy\"\n    .balign 2\n", "    la a0, ascii\n    li a1, 1\n", 0x200),
            1,
        ),
        c(
            "write 120 B CJK+ASCII",
            call_loop(&format!("    j 1f\ncjk: .ascii \"{cjk}\"\n    .balign 2\n"), &format!("    la a0, cjk\n    li a1, {}\n", cjk.len()), 0x200),
            1,
        ),
        c(
            "write newlines (scroll each)",
            call_loop(
                &format!("    li a0, 0\n    li a1, 29\n    li a7, 0x202\n    ecall\n{}", fill_ram(64, b'\n')),
                "    li a0, 0x100000\n    li a1, 64\n",
                0x200,
            ),
            1,
        ),
        c("command clear", call_loop("", "    li a0, 1\n", 0x201), 1),
        c("command scroll up", call_loop("", "    li a0, 2\n", 0x201), 1),
        c("cells_write 2400", call_loop("", "    li a0, 0\n    li a1, 0x100000\n    li a2, 2400\n", 0x207), 2),
        c("cells_write 8", call_loop("", "    li a0, 7\n    li a1, 0x100000\n    li a2, 8\n", 0x207), 1),
        c("cells_read 2400", call_loop("", "    li a0, 0\n    li a1, 0x100000\n    li a2, 2400\n", 0x206), 2),
        c("palette_set 256", call_loop("", "    li a0, 0\n    li a1, 0x100000\n    li a2, 256\n", 0x208), 1),
        // A list of lines, changing every call (one byte differs), so the
        // device copies it each time.
        c(
            "vector submit 16384",
            call_loop(&fill_ram(16384, 5), "    li a0, 0x100000\n    li a1, 16384\n    addi s0, s0, 1\n    sb s0, 1(a0)\n", 0x300),
            3,
        ),
        c(
            "vector submit 100",
            call_loop(&fill_ram(100, 5), "    li a0, 0x100000\n    li a1, 100\n    addi s0, s0, 1\n    sb s0, 1(a0)\n", 0x300),
            1,
        ),
        c("vector style", call_loop("", "    li a0, 1\n", 0x302), 1),
        c(
            "content put 16384",
            call_loop(&block(2, 16384), "    li a0, 0x100000\n    addi s0, s0, 1\n    li t0, 0x101000\n    sb s0, 0(t0)\n", 0x400),
            4,
        ),
        c(
            "content put 1 MiB - 8 KiB",
            call_loop(&block(2, 1040384), "    li a0, 0x100000\n    addi s0, s0, 1\n    li t0, 0x101000\n    sb s0, 0(t0)\n", 0x400),
            128,
        ),
        c(
            "content get (delivered 16384)",
            call_loop(&block(1, 0), "    li a0, 0x100000\n", 0x400),
            4,
        ),
    ]
}

/// The host's answers: puts succeed, gets deliver 16,384 bytes.
fn complete(m: &mut Machine) -> Vec<Completion> {
    m.take_content_requests()
        .into_iter()
        .map(|r| {
            let outcome = match r.kind {
                RequestKind::Put { data } => Outcome::Put { id: *blake3::hash(&data).as_bytes() },
                RequestKind::Get { .. } => Outcome::Got { size: 16384, data: vec![7; 16384] },
                _ => Outcome::Failed,
            };
            Completion { handle: r.handle, outcome }
        })
        .collect()
}

/// Runs `frames` frames; returns (host seconds in run_frame, units used).
fn run(m: &mut Machine, frames: u32, random: &[u8]) -> (f64, u64) {
    let (mut secs, mut units) = (0.0, 0);
    let mut completions = Vec::new();
    for _ in 0..frames {
        let input = FrameInput { random, completions: &completions, ..Default::default() };
        let t = Instant::now();
        let r = m.run_frame(&input);
        secs += t.elapsed().as_secs_f64();
        units += r.used;
        assert!(r.stopped.is_none(), "{:?}", r.stopped);
        completions = complete(m);
    }
    (secs, units)
}

fn machine(case: &Case) -> Machine {
    let rom = common::assemble(&case.src);
    Machine::new(&common::program(&rom, case.ram_pages, &[common::TERMINAL, common::VECTOR, common::CONTENT]))
}

#[test]
fn all_loops_run() {
    if !common::have_tools() {
        return;
    }
    for case in cases() {
        let mut m = machine(&case);
        let (_, units) = run(&mut m, 3, &[]);
        assert!(units >= 3 * 125_000, "{}: {units}", case.name);
        assert!(m.calls() > 0 || case.name.starts_with("plain"), "{}", case.name);
    }
}

#[test]
#[ignore]
fn measure() {
    if !common::have_tools() {
        return;
    }
    let random = vec![0x5A; MAX_RANDOM_PER_FRAME];
    let mut base = 0.0;
    println!("{:<32} {:>10} {:>10} {:>9} {:>7}", "frame spent in", "ns/unit", "us/frame", "calls/fr", "x ROM");
    for case in cases() {
        let mut best = f64::MAX;
        let mut per_frame = 0.0;
        let mut calls = 0.0;
        for _ in 0..5 {
            let mut m = machine(&case);
            run(&mut m, 20, &random);
            let c0 = m.calls();
            let (secs, units) = run(&mut m, 200, &random);
            let ns = secs * 1e9 / units as f64;
            if ns < best {
                best = ns;
                per_frame = secs * 1e6 / 200.0;
                calls = (m.calls() - c0) as f64 / 200.0;
            }
        }
        if case.name.starts_with("plain") {
            base = best;
        }
        println!("{:<32} {:>10.3} {:>10.1} {:>9.0} {:>7.2}", case.name, best, per_frame, calls, best / base);
    }
}
