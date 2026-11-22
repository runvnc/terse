use crate::cpu::{Cpu, RAM_BASE, ROM_BASE};

pub struct Device {
    pub slot: u8,
    pub version: u8,
    pub optional: bool,
}

pub fn load(file: &[u8]) -> Result<(Cpu, Vec<Device>), String> {
    if file.len() < 32 || &file[..6] != b"TERS\0\x01" {
        return Err("not a Terse32 program".into());
    }
    let rom_len =
        u32::from_le_bytes([file[6], file[7], file[8], 0]);
    let (pages, count) = (file[9] as usize, file[10] as usize);
    let size = 32 + 2 * count + rom_len as usize;
    if !(1..=262144).contains(&rom_len)
        || !(1..=96).contains(&pages)
        || count > 16
        || file.len() != size
    {
        return Err("a malformed program file".into());
    }
    let devices = file[32..32 + 2 * count]
        .chunks(2)
        .map(|d| Device {
            slot: d[0],
            version: d[1] & 0x7f,
            optional: d[1] & 0x80 != 0,
        })
        .collect();
    let mut cpu = Cpu {
        x: [0; 32],
        pc: ROM_BASE,
        rom: file[32 + 2 * count..].to_vec(),
        ram: vec![0; pages * 8192],
    };
    cpu.x[2] = RAM_BASE + cpu.ram.len() as u32;
    Ok((cpu, devices))
}
