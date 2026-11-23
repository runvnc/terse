use crate::cpu::{Cpu, RAM_BASE, ROM_BASE, SP};

const HEADER_SIZE: usize = 32;
const MAGIC: &[u8] = b"TERS\0\x01";
const ROM_LEN_AT: usize = 6;
const RAM_PAGES_AT: usize = 9;
const DEVICES_AT: usize = 10;
const MAX_ROM: usize = 262144;
const MAX_RAM_PAGES: usize = 96;
const MAX_DEVICES: usize = 16;
pub const DEVICE_SIZE: usize = 2;
pub const PAGE_SIZE: usize = 8192;
const OPTIONAL: u8 = 0x80;

pub struct Device {
    pub slot: u8,
    pub version: u8,
    pub optional: bool,
}

pub fn load(file: &[u8]) -> Result<(Cpu, Vec<Device>), String> {
    if file.len() < HEADER_SIZE || !file.starts_with(MAGIC) {
        return Err("not a Terse32 program".into());
    }
    let r = &file[ROM_LEN_AT..];
    let rom_len =
        u32::from_le_bytes([r[0], r[1], r[2], 0]) as usize;
    let pages = file[RAM_PAGES_AT] as usize;
    let count = file[DEVICES_AT] as usize;
    let rom_at = HEADER_SIZE + DEVICE_SIZE * count;
    if !(1..=MAX_ROM).contains(&rom_len)
        || !(1..=MAX_RAM_PAGES).contains(&pages)
        || count > MAX_DEVICES
        || file.len() != rom_at + rom_len
    {
        return Err("a malformed program file".into());
    }
    let devices = file[HEADER_SIZE..rom_at]
        .chunks(DEVICE_SIZE)
        .map(|d| Device {
            slot: d[0],
            version: d[1] & !OPTIONAL,
            optional: d[1] & OPTIONAL != 0,
        })
        .collect();
    let mut cpu = Cpu {
        x: [0; 32],
        pc: ROM_BASE,
        rom: file[rom_at..].to_vec(),
        ram: vec![0; pages * PAGE_SIZE],
    };
    cpu.x[SP] = RAM_BASE + cpu.ram.len() as u32;
    Ok((cpu, devices))
}
