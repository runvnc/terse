//! Doubles without an FPU: tests/softfloat/softfloat.c (add, subtract,
//! multiply, divide) is checked bit for bit against this host's FPU on two
//! million random operand pairs (natively), then run on the machine, where it
//! must give the native checksum; prints the instructions per operation.

mod common;

use std::path::Path;
use std::process::Command;

use terse_core::Machine;

#[test]
fn soft_doubles() {
    if !common::have_tools() {
        return;
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/softfloat");
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("softfloat-native");
    let out = Command::new("clang")
        .args(["-O2", "-fwrapv", "-DTERSE_NATIVE", "-o"])
        .arg(&exe)
        .arg(src.join("main.c"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(&exe).output().unwrap();
    let native = String::from_utf8(out.stdout).unwrap();
    assert!(native.starts_with("mismatches 0\n"), "{native}{}", String::from_utf8_lossy(&out.stderr));
    let checksum = native.lines().nth(1).unwrap();

    for opt in ["-Os", "-O2"] {
        let rom = common::build(&[src.join("main.c")], opt, true);
        let mut m = Machine::new(&common::program(&rom, 4, &[common::TERMINAL]));
        let mut costs = Vec::new();
        while m.exit_code().is_none() && costs.len() < 100 {
            costs.push(m.run_frame(&Default::default()).used);
        }
        assert_eq!(m.exit_code(), Some(0), "{:?}", m.fault());
        assert_eq!(m.terminal().unwrap().text().lines().next().unwrap(), checksum, "{opt}");
        // The last frames: the loop alone, adds, multiplies, divides, exit.
        let base = costs.len() - 5;
        let per = |k: usize| (costs[base + k - 1] - costs[base]) as f64 / 200.0;
        eprintln!(
            "soft doubles {opt}: {:.0} instructions an add, {:.0} a multiply, {:.0} a divide (ROM {} bytes)",
            per(2),
            per(3),
            per(4),
            rom.len()
        );
    }
}
