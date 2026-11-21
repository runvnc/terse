//! Helpers for the machine tests: finding the RISC-V tools (tests that
//! need them are skipped with a message when they are missing), building
//! guest code, and running images.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use terse_core::{DeviceReq, FrameInput, FrameResult, Image, Machine};

pub fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

pub fn rv_lib() -> PathBuf {
    workspace().join("programs/lib")
}

/// Whether clang (with the riscv32 target), ld.lld, llvm-objcopy and llvm-mc
/// all work. Printed once when they do not.
pub fn have_tools() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("t.c");
        std::fs::write(&src, "int f(int a) { return a * 3; }\n").unwrap();
        let cc = Command::new("clang")
            .args(["--target=riscv32-unknown-elf", "-march=rv32imc", "-mabi=ilp32", "-c"])
            .arg(&src)
            .arg("-o")
            .arg(dir.path().join("t.o"))
            .output();
        let ok = cc.is_ok_and(|o| o.status.success())
            && ["ld.lld", "llvm-objcopy", "llvm-mc"]
                .iter()
                .all(|t| Command::new(t).arg("--version").output().is_ok_and(|o| o.status.success()));
        if !ok {
            eprintln!("skipped: needs clang with the riscv32 target, ld.lld, llvm-objcopy and llvm-mc");
        }
        ok
    })
}

fn run(cmd: &mut Command) {
    let out = cmd.output().unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
    assert!(out.status.success(), "{cmd:?}\n{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}

/// Compiles C and assembly sources for rv32imc with the crt0,
/// library and linker script; returns the raw ROM. `opt` is e.g. "-Os".
pub fn build(sources: &[PathBuf], opt: &str, with_lib: bool) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let lib = rv_lib();
    let mut objs = Vec::new();
    let mut all: Vec<PathBuf> = sources.to_vec();
    if with_lib {
        all.insert(0, lib.join("crt0.S"));
        all.push(lib.join("terse.c"));
    }
    for (i, src) in all.iter().enumerate() {
        let obj = dir.path().join(format!("{i}.o"));
        run(Command::new("clang")
            .args(["--target=riscv32-unknown-elf", "-march=rv32imc", "-mabi=ilp32", opt])
            .args(["-ffreestanding", "-nostdlib", "-fno-builtin", "-ffunction-sections", "-fdata-sections"])
            .arg("-I")
            .arg(&lib)
            .arg("-c")
            .arg(src)
            .arg("-o")
            .arg(&obj));
        objs.push(obj);
    }
    let elf = dir.path().join("out.elf");
    let bin = dir.path().join("out.bin");
    run(Command::new("ld.lld").arg("-T").arg(lib.join("terse.ld")).arg("--gc-sections").args(&objs).arg("-o").arg(&elf));
    run(Command::new("llvm-objcopy").args(["-O", "binary"]).arg(&elf).arg(&bin));
    std::fs::read(&bin).unwrap()
}

/// Assembles a source text (GNU syntax, starting at ROM_BASE) without the library.
pub fn assemble(src: &str) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.S");
    std::fs::write(&path, format!("    .section .text.start, \"ax\"\n    .globl _start\n_start:\n{src}\n")).unwrap();
    build(&[path], "-O0", false)
}

pub const TERMINAL: DeviceReq = DeviceReq::new(2, 0);
pub const VECTOR: DeviceReq = DeviceReq::new(3, 0);
pub const CONTENT: DeviceReq = DeviceReq::new(4, 0);
#[allow(dead_code)]
pub const PAGE: DeviceReq = DeviceReq::new(7, 0);

pub fn program(rom: &[u8], ram_pages: u8, devices: &[DeviceReq]) -> Image {
    Image::parse(&Image::build("test", ram_pages, devices, rom)).unwrap()
}

pub fn machine(rom: &[u8], devices: &[DeviceReq]) -> Machine {
    Machine::new(&program(rom, 4, devices))
}

pub fn frame(m: &mut Machine) -> FrameResult {
    m.run_frame(&FrameInput::default())
}

/// Runs until the program exits; panics after `max` frames or on a fault.
pub fn run_to_exit(m: &mut Machine, max: u32) -> u8 {
    for _ in 0..max {
        if let Some(s) = m.run_frame(&FrameInput::default()).stopped {
            match s {
                terse_core::Stopped::Exit(code) => return code,
                terse_core::Stopped::Fault(f) => panic!("fault: {f}"),
                terse_core::Stopped::Halted(s) => panic!("halted for device {s}"),
            }
        }
    }
    panic!("no exit within {max} frames");
}
