//! Paged memory (SPEC 1.1) and device 0, the page registers (SPEC 4).

pub const PAGE_SIZE: usize = 0x2000;
pub const SLOTS: usize = 8;
const RAM_BASE: u8 = 0x80;
const ROM_PAGES: u8 = 0x10;

pub struct Memory {
    slots: [u8; SLOTS],
    rom: Vec<u8>,
    ram: Vec<u8>,
}

enum Target {
    Rom(usize),
    Ram(usize),
    Unmapped,
}

impl Memory {
    pub fn new(rom: &[u8], ram_pages: u8) -> Self {
        Memory {
            // SPEC 1.2: slots 0-3 show ROM 00h-03h, slots 4-7 show RAM 80h-83h.
            slots: [0x00, 0x01, 0x02, 0x03, 0x80, 0x81, 0x82, 0x83],
            rom: rom.to_vec(),
            ram: vec![0; ram_pages as usize * PAGE_SIZE],
        }
    }

    fn target(&self, addr: u16) -> Target {
        let page = self.slots[addr as usize / PAGE_SIZE];
        let offset = addr as usize % PAGE_SIZE;
        if page < ROM_PAGES {
            let i = page as usize * PAGE_SIZE + offset;
            if i < self.rom.len() { Target::Rom(i) } else { Target::Unmapped }
        } else if page >= RAM_BASE {
            let i = (page - RAM_BASE) as usize * PAGE_SIZE + offset;
            if i < self.ram.len() { Target::Ram(i) } else { Target::Unmapped }
        } else {
            Target::Unmapped
        }
    }

    pub fn read(&self, addr: u16) -> u8 {
        match self.target(addr) {
            Target::Rom(i) => self.rom[i],
            Target::Ram(i) => self.ram[i],
            Target::Unmapped => 0xFF,
        }
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        if let Target::Ram(i) = self.target(addr) {
            self.ram[i] = value;
        }
    }

    /// Ports 00h-07h; 08h-0Fh are reserved (SPEC 4).
    pub fn port_in(&self, reg: u8) -> u8 {
        self.slots.get(reg as usize).copied().unwrap_or(0xFF)
    }

    pub fn port_out(&mut self, reg: u8, value: u8) {
        if let Some(slot) = self.slots.get_mut(reg as usize) {
            *slot = value;
        }
    }

    pub fn slots(&self) -> &[u8; SLOTS] {
        &self.slots
    }

    pub fn ram(&self) -> &[u8] {
        &self.ram
    }

    pub fn ram_mut(&mut self) -> &mut [u8] {
        &mut self.ram
    }

    /// Slot registers and RAM (STATE.md, memory section).
    pub(crate) fn write_state(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.slots);
        out.extend_from_slice(&(self.ram.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.ram);
    }

    /// The linear RAM address (SPEC 7.1) that `addr` shows in the current
    /// mapping, or `None` if it does not show RAM.
    pub fn linear(&self, addr: u16) -> Option<usize> {
        match self.target(addr) {
            Target::Ram(i) => Some(i),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_mapping_and_rom() {
        let mut m = Memory::new(&[1, 2, 3], 4);
        assert_eq!(m.read(0), 1);
        assert_eq!(m.read(2), 3);
        assert_eq!(m.read(3), 0xFF, "beyond end of image");
        assert_eq!(m.read(0x2000), 0xFF, "ROM page 1 beyond image");
        m.write(0, 0x55);
        assert_eq!(m.read(0), 1, "ROM write ignored");
        assert_eq!(m.read(0x8000), 0, "RAM zeroed");
        m.write(0x8000, 0x42);
        m.write(0xFFFF, 0x43);
        assert_eq!(m.read(0x8000), 0x42);
        assert_eq!(m.read(0xFFFF), 0x43);
        assert_eq!(m.ram()[0], 0x42);
        assert_eq!(m.ram()[4 * PAGE_SIZE - 1], 0x43);
    }

    #[test]
    fn paging() {
        let mut m = Memory::new(&[9], 4);
        m.write(0x8000, 7);
        // Show RAM page 80h in slot 0 too: aliases slot 4.
        m.port_out(0, 0x80);
        assert_eq!(m.port_in(0), 0x80);
        assert_eq!(m.read(0x0000), 7);
        m.write(0x0001, 8);
        assert_eq!(m.read(0x8001), 8);
        // Unmapped page numbers.
        m.port_out(1, 0x10);
        assert_eq!(m.read(0x2000), 0xFF);
        m.port_out(1, 0x7F);
        m.write(0x2000, 1);
        assert_eq!(m.read(0x2000), 0xFF);
        // RAM beyond declared size (4 pages: 80h-83h) is unmapped.
        m.port_out(2, 0x84);
        m.write(0x4000, 1);
        assert_eq!(m.read(0x4000), 0xFF);
        m.port_out(2, 0x83);
        m.write(0x4000, 1);
        assert_eq!(m.read(0x4000), 1);
        assert_eq!(m.read(0xE000), 1, "slot 7 also shows 83h");
        // Reserved ports.
        assert_eq!(m.port_in(8), 0xFF);
        m.port_out(8, 1);
        assert_eq!(m.slots(), &[0x80, 0x7F, 0x83, 0x03, 0x80, 0x81, 0x82, 0x83]);
    }

    #[test]
    fn rom_pages_and_ram_size() {
        let rom: Vec<u8> = (0..3 * PAGE_SIZE).map(|i| (i / PAGE_SIZE) as u8 + 1).collect();
        let mut m = Memory::new(&rom, 128);
        m.port_out(0, 2);
        assert_eq!(m.read(0x0000), 3);
        m.port_out(0, 0xFF);
        m.write(0x10, 5);
        assert_eq!(m.read(0x10), 5);
        assert_eq!(m.ram()[127 * PAGE_SIZE + 0x10], 5);
    }
}
