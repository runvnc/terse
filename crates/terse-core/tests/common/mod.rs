#![allow(dead_code)]

use terse_core::{DeviceReq, FrameInput, FrameResult, Image, Machine};

/// Assembles Z80 source (Zilog syntax) into a flat image starting at 0000h.
pub fn asm(src: &str) -> Vec<u8> {
    let arch = rsasm::arch::lookup("z80").unwrap();
    let options = rsasm::assembler::Options::new()
        .with_dialect(arch.default_dialect())
        .with_relocatable(false);
    let mut a = rsasm::assembler::Assembler::new(arch, options);
    a.assemble_str("test.asm", src);
    let ok = a.finish();
    assert!(ok && !a.diags().has_errors(), "{}", a.diags().render(a.source_map(), false));
    rsasm::output::raw::build(&a).unwrap()
}

pub fn machine_with(rom: &[u8], devices: &[DeviceReq]) -> Machine {
    let file = Image::build("test", 4, devices, rom);
    Machine::new(&Image::parse(&file).unwrap())
}

pub const TERMINAL: DeviceReq = DeviceReq { slot: 2, version: 0 };

pub fn machine(src: &str) -> Machine {
    machine_with(&asm(src), &[TERMINAL])
}

pub fn frame(m: &mut Machine) -> FrameResult {
    m.run_frame(&FrameInput::default())
}
