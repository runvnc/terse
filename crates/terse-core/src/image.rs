//! Image file format (SPEC 2).

use std::fmt;

pub const MAGIC: [u8; 4] = *b"TERS";
pub const FORMAT_VERSION: u8 = 0;
pub const PROFILE: u8 = 1;
pub const HEADER_LEN: usize = 32;
pub const ROM_MAX: usize = 131_072;
pub const RAM_PAGES_MIN: u8 = 4;
pub const RAM_PAGES_MAX: u8 = 128;
pub const MAX_DEVICES: usize = 16;
pub const TITLE_LEN: usize = 16;

/// Device slots and versions this implementation provides (SPEC 3).
pub const SUPPORTED_DEVICES: [DeviceReq; 5] = [
    DeviceReq { slot: 0, version: 0 },
    DeviceReq { slot: 1, version: 0 },
    DeviceReq { slot: 2, version: 0 },
    DeviceReq { slot: 3, version: 0 },
    DeviceReq { slot: 4, version: 0 },
];

/// A (device slot, device version) requirement from the image header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceReq {
    pub slot: u8,
    pub version: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageError {
    TooShort { len: usize },
    BadMagic,
    UnsupportedFormatVersion(u8),
    UnsupportedProfile(u8),
    RomLength(usize),
    RamPages(u8),
    DeviceCount(u8),
    ReservedNotZero,
    LengthMismatch { expected: usize, actual: usize },
    UnsupportedDevice(DeviceReq),
    DuplicateDevice(u8),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { len } => write!(f, "file is {len} bytes, shorter than the 32-byte header"),
            Self::BadMagic => write!(f, "not a Terse image (bad magic)"),
            Self::UnsupportedFormatVersion(v) => write!(f, "unsupported format version {v}"),
            Self::UnsupportedProfile(p) => write!(f, "unsupported machine profile {p}"),
            Self::RomLength(n) => write!(f, "ROM length {n} outside 1..={ROM_MAX}"),
            Self::RamPages(n) => {
                write!(f, "RAM pages {n} outside {RAM_PAGES_MIN}..={RAM_PAGES_MAX}")
            }
            Self::DeviceCount(n) => write!(f, "device count {n} above {MAX_DEVICES}"),
            Self::ReservedNotZero => write!(f, "reserved header bytes are not zero"),
            Self::LengthMismatch { expected, actual } => {
                write!(f, "file length {actual} does not match header ({expected})")
            }
            Self::UnsupportedDevice(d) => {
                write!(f, "device {} version {} is not implemented", d.slot, d.version)
            }
            Self::DuplicateDevice(s) => write!(f, "device slot {s} listed twice"),
        }
    }
}

impl std::error::Error for ImageError {}

/// A validated program image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// Title bytes with the zero padding removed (UTF-8 per the spec, not checked).
    pub title: Vec<u8>,
    pub ram_pages: u8,
    pub devices: Vec<DeviceReq>,
    pub rom: Vec<u8>,
    /// BLAKE3-256 of the whole file (SPEC 2).
    pub id: [u8; 32],
}

impl Image {
    pub fn parse(file: &[u8]) -> Result<Self, ImageError> {
        if file.len() < HEADER_LEN {
            return Err(ImageError::TooShort { len: file.len() });
        }
        if file[0..4] != MAGIC {
            return Err(ImageError::BadMagic);
        }
        if file[4] != FORMAT_VERSION {
            return Err(ImageError::UnsupportedFormatVersion(file[4]));
        }
        if file[5] != PROFILE {
            return Err(ImageError::UnsupportedProfile(file[5]));
        }
        let rom_len = file[6] as usize | (file[7] as usize) << 8 | (file[8] as usize) << 16;
        if !(1..=ROM_MAX).contains(&rom_len) {
            return Err(ImageError::RomLength(rom_len));
        }
        let ram_pages = file[9];
        if !(RAM_PAGES_MIN..=RAM_PAGES_MAX).contains(&ram_pages) {
            return Err(ImageError::RamPages(ram_pages));
        }
        let n_dev = file[10];
        if n_dev as usize > MAX_DEVICES {
            return Err(ImageError::DeviceCount(n_dev));
        }
        if file[11..16].iter().any(|&b| b != 0) {
            return Err(ImageError::ReservedNotZero);
        }
        let rom_start = HEADER_LEN + 2 * n_dev as usize;
        let expected = rom_start + rom_len;
        if file.len() != expected {
            return Err(ImageError::LengthMismatch { expected, actual: file.len() });
        }

        let mut devices: Vec<DeviceReq> = Vec::with_capacity(n_dev as usize);
        for pair in file[HEADER_LEN..rom_start].chunks_exact(2) {
            let req = DeviceReq { slot: pair[0], version: pair[1] };
            if devices.iter().any(|d| d.slot == req.slot) {
                return Err(ImageError::DuplicateDevice(req.slot));
            }
            if !SUPPORTED_DEVICES.contains(&req) {
                return Err(ImageError::UnsupportedDevice(req));
            }
            devices.push(req);
        }

        let title = &file[16..32];
        let title_end = title.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        Ok(Image {
            title: title[..title_end].to_vec(),
            ram_pages,
            devices,
            rom: file[rom_start..].to_vec(),
            id: *blake3::hash(file).as_bytes(),
        })
    }

    /// Builds the file bytes for an image (the inverse of `parse`).
    pub fn build(title: &str, ram_pages: u8, devices: &[DeviceReq], rom: &[u8]) -> Vec<u8> {
        let title = title.as_bytes();
        let mut out = Vec::with_capacity(HEADER_LEN + 2 * devices.len() + rom.len());
        out.extend_from_slice(&MAGIC);
        out.push(FORMAT_VERSION);
        out.push(PROFILE);
        out.extend_from_slice(&(rom.len() as u32).to_le_bytes()[..3]);
        out.push(ram_pages);
        out.push(devices.len() as u8);
        out.extend_from_slice(&[0; 5]);
        let mut t = [0u8; TITLE_LEN];
        let n = title.len().min(TITLE_LEN);
        t[..n].copy_from_slice(&title[..n]);
        out.extend_from_slice(&t);
        for d in devices {
            out.extend_from_slice(&[d.slot, d.version]);
        }
        out.extend_from_slice(rom);
        out
    }

    pub fn has_device(&self, slot: u8) -> bool {
        slot <= 1 || self.devices.iter().any(|d| d.slot == slot)
    }

    pub fn title_lossy(&self) -> String {
        String::from_utf8_lossy(&self.title).into_owned()
    }

    pub fn id_hex(&self) -> String {
        self.id.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> Vec<u8> {
        Image::build("hi", 4, &[DeviceReq { slot: 2, version: 0 }], &[0x76])
    }

    #[test]
    fn round_trip() {
        let file = valid();
        let img = Image::parse(&file).unwrap();
        assert_eq!(img.title, b"hi");
        assert_eq!(img.ram_pages, 4);
        assert_eq!(img.rom, [0x76]);
        assert!(img.has_device(0) && img.has_device(1) && img.has_device(2));
        assert!(!img.has_device(3));
        assert_eq!(img.id, *blake3::hash(&file).as_bytes());
    }

    #[test]
    fn rejects_invalid_headers() {
        let check = |f: &dyn Fn(&mut Vec<u8>), want: ImageError| {
            let mut file = valid();
            f(&mut file);
            assert_eq!(Image::parse(&file), Err(want));
        };
        check(&|f| f.truncate(10), ImageError::TooShort { len: 10 });
        check(&|f| f[0] = b'X', ImageError::BadMagic);
        check(&|f| f[4] = 1, ImageError::UnsupportedFormatVersion(1));
        check(&|f| f[5] = 2, ImageError::UnsupportedProfile(2));
        check(&|f| f[6] = 0, ImageError::RomLength(0));
        check(&|f| f[8] = 2, ImageError::RomLength(0x20001));
        check(&|f| f[9] = 3, ImageError::RamPages(3));
        check(&|f| f[9] = 129, ImageError::RamPages(129));
        check(&|f| f[10] = 17, ImageError::DeviceCount(17));
        check(&|f| f[15] = 1, ImageError::ReservedNotZero);
        check(&|f| f.push(0), ImageError::LengthMismatch { expected: 35, actual: 36 });
        check(&|f| f[33] = 1, ImageError::UnsupportedDevice(DeviceReq { slot: 2, version: 1 }));
        check(&|f| f[32] = 5, ImageError::UnsupportedDevice(DeviceReq { slot: 5, version: 0 }));
        check(&|f| f[32] = 5, ImageError::UnsupportedDevice(DeviceReq { slot: 5, version: 0 }));
    }

    #[test]
    fn rejects_duplicate_device() {
        let d = DeviceReq { slot: 2, version: 0 };
        let file = Image::build("", 4, &[d, d], &[0]);
        assert_eq!(Image::parse(&file), Err(ImageError::DuplicateDevice(2)));
    }

    #[test]
    fn max_rom_accepted() {
        let file = Image::build("", 128, &[], &vec![0; ROM_MAX]);
        assert_eq!(Image::parse(&file).unwrap().rom.len(), ROM_MAX);
    }
}
