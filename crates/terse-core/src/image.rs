//! Image file format (SPEC §image). Profile 1 is the RV32IMC machine.

use std::fmt;

pub const MAGIC: [u8; 4] = *b"TERS";
pub const FORMAT_VERSION: u8 = 0;
pub const PROFILE: u8 = 1;
pub const HEADER_LEN: usize = 32;
/// ROM's largest size (SPEC §profile): it ends by 0005_0000h.
pub const ROM_MAX: usize = 262_144;
pub const RAM_PAGES_MIN: u8 = 1;
/// RAM's largest size in 8 KiB pages (768 KiB): it ends by 001C_0000h,
/// and ROM and RAM together are at most 1 MiB.
pub const RAM_PAGES_MAX: u8 = 96;
pub const MAX_DEVICES: usize = 16;
pub const TITLE_LEN: usize = 16;
/// The top bit of a device entry's version byte: the device is optional
/// (SPEC §image); the version is the other seven bits, 0 to 127.
pub const OPTIONAL: u8 = 0x80;

/// The experimental slots (`spec/experimental.md`): 240 to 255. A device
/// on one is named in the image's experimental device table and served
/// only by a driver on a device bus.
pub const EXPERIMENTAL: std::ops::RangeInclusive<u8> = 240..=255;
/// The longest name of an experimental device.
pub const EXPERIMENTAL_NAME_MAX: usize = 30;
/// The longest state section of an experimental device (STATE.md 1.13).
pub const EXPERIMENTAL_STATE_MAX: usize = 16_384;
/// The end of the experimental device table, the last 3 bytes of the ROM.
pub const EXPERIMENTAL_MAGIC: [u8; 3] = *b"TXD";
/// Bytes of one entry of the table.
const EXPERIMENTAL_ENTRY: usize = 32;

/// Whether `name` may name an experimental device: 1 to 30 of `a`-`z`,
/// `0`-`9`, `-` and `.`, starting with a letter.
pub fn valid_experimental_name(name: &str) -> bool {
    (1..=EXPERIMENTAL_NAME_MAX).contains(&name.len())
        && name.as_bytes()[0].is_ascii_lowercase()
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
}

/// The experimental device table for `entries` (slot, name), the bytes
/// `terse pack` puts at the end of the ROM (`spec/experimental.md`
/// §experimental.image). The caller checks the names and slots.
pub fn experimental_table(entries: &[(u8, String)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(EXPERIMENTAL_ENTRY * entries.len() + 4);
    for (slot, name) in entries {
        let mut e = [0u8; EXPERIMENTAL_ENTRY];
        e[0] = *slot;
        e[1] = name.len() as u8;
        e[2..2 + name.len()].copy_from_slice(name.as_bytes());
        out.extend_from_slice(&e);
    }
    out.push(entries.len() as u8);
    out.extend_from_slice(&EXPERIMENTAL_MAGIC);
    out
}

/// The end of an image's blob list (SPEC §image.blobs): its last 3 bytes.
pub const BLOB_LIST_MAGIC: [u8; 3] = *b"TBL";
/// The most blobs an image may list.
pub const BLOB_LIST_MAX: usize = 64;
/// The most bytes the blobs an image lists may total (4 MiB).
pub const BLOB_LIST_TOTAL: u64 = 4 * 1024 * 1024;
/// The largest blob (NETWORK.md section 1).
pub const BLOB_SIZE_MAX: u32 = 1_048_576;
/// Bytes of one entry of the list: the blob's ID and its size.
const BLOB_LIST_ENTRY: usize = 36;

/// A blob an image lists (SPEC §image.blobs): its ID and size in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListedBlob {
    pub id: [u8; 32],
    pub size: u32,
}

/// Why a blob list cannot be made: too many blobs, a size out of range,
/// too many bytes in all, or a blob twice.
pub fn check_blob_list(blobs: &[ListedBlob]) -> Result<(), String> {
    if blobs.is_empty() || blobs.len() > BLOB_LIST_MAX {
        return Err(format!("an image lists 1 to {BLOB_LIST_MAX} blobs, not {}", blobs.len()));
    }
    if let Some(b) = blobs.iter().find(|b| !(1..=BLOB_SIZE_MAX).contains(&b.size)) {
        return Err(format!("a blob is 1 to {BLOB_SIZE_MAX} bytes, not {}", b.size));
    }
    let total: u64 = blobs.iter().map(|b| b.size as u64).sum();
    if total > BLOB_LIST_TOTAL {
        return Err(format!("the blobs an image lists total at most {BLOB_LIST_TOTAL} bytes; these are {total}"));
    }
    for (i, b) in blobs.iter().enumerate() {
        if blobs[..i].iter().any(|a| a.id == b.id) {
            return Err("a blob is listed twice".into());
        }
    }
    Ok(())
}

/// The blob list for `blobs`, the bytes `terse pack` puts at the end of the
/// ROM, before any experimental device table (SPEC §image.blobs). The
/// caller checks it ([`check_blob_list`]).
pub fn blob_list_table(blobs: &[ListedBlob]) -> Vec<u8> {
    let mut out = Vec::with_capacity(BLOB_LIST_ENTRY * blobs.len() + 4);
    for b in blobs {
        out.extend_from_slice(&b.id);
        out.extend_from_slice(&b.size.to_le_bytes());
    }
    out.push(blobs.len() as u8);
    out.extend_from_slice(&BLOB_LIST_MAGIC);
    out
}

/// Device slots and versions this implementation provides (SPEC §image).
pub const SUPPORTED_DEVICES: [DeviceReq; 9] = [
    DeviceReq::new(1, 0),
    DeviceReq::new(2, 0),
    DeviceReq::new(3, 0),
    DeviceReq::new(4, 0),
    DeviceReq::new(6, 0),
    DeviceReq::new(7, 0),
    DeviceReq::new(8, 0),
    DeviceReq::new(9, 0),
    // Full text, a capability (SPEC §terminal.text).
    DeviceReq::new(crate::text::FULL_TEXT, 0),
];

/// A device entry of the image header (SPEC §image): a slot, a version (0 to
/// 127) and whether the device is required or optional.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceReq {
    pub slot: u8,
    pub version: u8,
    /// Use the device if the host has it, run without it otherwise.
    pub optional: bool,
}

impl DeviceReq {
    /// A required device.
    pub const fn new(slot: u8, version: u8) -> Self {
        DeviceReq { slot, version, optional: false }
    }

    /// An optional device.
    pub const fn optional(slot: u8, version: u8) -> Self {
        DeviceReq { slot, version, optional: true }
    }

    /// Whether this implementation provides the device in this version.
    pub fn supported(&self) -> bool {
        SUPPORTED_DEVICES.iter().any(|d| d.slot == self.slot && d.version == self.version)
    }

    /// Whether the slot is an experimental one (240-255).
    pub fn experimental(&self) -> bool {
        EXPERIMENTAL.contains(&self.slot)
    }

    /// The entry's second byte: the version, with the top bit set if optional.
    pub fn version_byte(&self) -> u8 {
        self.version | if self.optional { OPTIONAL } else { 0 }
    }
}

/// Which of an image's optional devices a machine was given (SPEC §image,
/// STATE.md 1.5): one flag per optional entry, in header order. Fixed at
/// reset; part of the machine's state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grants(pub Vec<bool>);

/// The device slots a machine has: device 1, the image's required devices
/// and the optional ones it was granted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceSet([bool; 256]);

impl DeviceSet {
    pub fn has(&self, slot: u8) -> bool {
        self.0[slot as usize]
    }
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
    /// Device 1 or 8 listed as optional (SPEC §image).
    NotOptional(u8),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { len } => write!(f, "file is {len} bytes, shorter than the 32-byte header"),
            Self::BadMagic => write!(f, "not a Terse32 image (bad magic)"),
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
            Self::NotOptional(s) => write!(f, "device {s} cannot be listed as optional"),
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
    /// BLAKE3-256 of the whole file (SPEC §image).
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
            let req = DeviceReq { slot: pair[0], version: pair[1] & !OPTIONAL, optional: pair[1] & OPTIONAL != 0 };
            if devices.iter().any(|d| d.slot == req.slot) {
                return Err(ImageError::DuplicateDevice(req.slot));
            }
            // Device 1 is always there; device 8 is the shell's (SPEC §machines.shell).
            if req.optional && (req.slot == 1 || req.slot == crate::machines::SLOT) {
                return Err(ImageError::NotOptional(req.slot));
            }
            // A host refuses an image only for a required device it lacks;
            // an optional one it lacks is simply not granted.
            if !req.optional && !req.supported() {
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
            out.extend_from_slice(&[d.slot, d.version_byte()]);
        }
        out.extend_from_slice(rom);
        out
    }

    /// Whether the header lists device `slot`, required or optional
    /// (device 1 always counts as listed).
    pub fn lists(&self, slot: u8) -> bool {
        slot == 1 || self.devices.iter().any(|d| d.slot == slot)
    }

    /// Whether the header lists device `slot` as required.
    pub fn requires(&self, slot: u8) -> bool {
        slot == 1 || self.devices.iter().any(|d| d.slot == slot && !d.optional)
    }

    /// The optional entries, in header order.
    pub fn optional(&self) -> impl Iterator<Item = &DeviceReq> {
        self.devices.iter().filter(|d| d.optional)
    }

    /// The grants of a machine on a host that provides the devices
    /// `provides` accepts (and this implementation supports): an optional
    /// device is granted exactly when both do.
    pub fn grants(&self, provides: impl Fn(&DeviceReq) -> bool) -> Grants {
        Grants(self.optional().map(|d| d.supported() && provides(d)).collect())
    }

    /// The image's experimental devices (slot, name), in header order, if
    /// its header lists any and its experimental device table is valid
    /// (`spec/experimental.md` §experimental.image); `None` if the table
    /// is invalid or missing, an empty list if it lists none.
    pub fn experimental(&self) -> Option<Vec<(u8, String)>> {
        let slots: Vec<u8> = self.devices.iter().filter(|d| d.experimental()).map(|d| d.slot).collect();
        if slots.is_empty() {
            return Some(Vec::new());
        }
        let rom = &self.rom;
        let n = rom.len();
        if n < 4 || rom[n - 3..] != EXPERIMENTAL_MAGIC {
            return None;
        }
        let k = rom[n - 4] as usize;
        if k != slots.len() || n < 4 + EXPERIMENTAL_ENTRY * k {
            return None;
        }
        let table = &rom[n - 4 - EXPERIMENTAL_ENTRY * k..n - 4];
        let mut names: Vec<(u8, String)> = Vec::new();
        for e in table.chunks_exact(EXPERIMENTAL_ENTRY) {
            let len = e[1] as usize;
            if !(1..=EXPERIMENTAL_NAME_MAX).contains(&len) || e[2 + len..].iter().any(|&b| b != 0) {
                return None;
            }
            let name = std::str::from_utf8(&e[2..2 + len]).ok().filter(|s| valid_experimental_name(s))?;
            if !slots.contains(&e[0]) || names.iter().any(|(s, m)| *s == e[0] || m == name) {
                return None;
            }
            names.push((e[0], name.to_string()));
        }
        // In header order.
        Some(slots.iter().map(|s| names.iter().find(|(t, _)| t == s).cloned().unwrap()).collect())
    }

    /// The blobs the image lists (SPEC §image.blobs), in its order: empty if
    /// it has no list or an invalid one. The list ends where the
    /// experimental device table begins, if the header lists experimental
    /// slots, else at the end of the ROM.
    pub fn blob_list(&self) -> Vec<ListedBlob> {
        let k = self.devices.iter().filter(|d| d.experimental()).count();
        let table = if k == 0 { 0 } else { 4 + EXPERIMENTAL_ENTRY * k };
        let Some(end) = self.rom.len().checked_sub(table) else { return Vec::new() };
        let rom = &self.rom[..end];
        if end < 4 || rom[end - 3..] != BLOB_LIST_MAGIC {
            return Vec::new();
        }
        let n = rom[end - 4] as usize;
        let Some(start) = (end - 4).checked_sub(BLOB_LIST_ENTRY * n) else { return Vec::new() };
        let blobs: Vec<ListedBlob> = rom[start..end - 4]
            .chunks_exact(BLOB_LIST_ENTRY)
            .map(|e| ListedBlob { id: e[..32].try_into().unwrap(), size: u32::from_le_bytes(e[32..36].try_into().unwrap()) })
            .collect();
        if check_blob_list(&blobs).is_err() {
            return Vec::new();
        }
        blobs
    }

    /// The name of the experimental device on `slot`, if the image lists one
    /// there and its table is valid.
    pub fn experimental_name(&self, slot: u8) -> Option<String> {
        self.experimental()?.into_iter().find(|(s, _)| *s == slot).map(|(_, n)| n)
    }

    /// [`Image::grants`] on a host that also grants experimental devices
    /// (a device bus): an optional experimental device is granted when the
    /// image's table is valid and `experimental` accepts it with its name;
    /// every other one as [`Image::grants`] decides.
    pub fn grants_on(&self, provides: impl Fn(&DeviceReq) -> bool, experimental: impl Fn(&DeviceReq, &str) -> bool) -> Grants {
        let names = self.experimental().unwrap_or_default();
        Grants(
            self.optional()
                .map(|d| match d.experimental() {
                    true => names.iter().find(|(s, _)| *s == d.slot).is_some_and(|(_, n)| experimental(d, n)),
                    false => d.supported() && provides(d),
                })
                .collect(),
        )
    }

    /// Whether `grants` grants one of the image's experimental devices.
    pub fn grants_experimental(&self, grants: &Grants) -> bool {
        self.optional().zip(&grants.0).any(|(d, g)| *g && d.experimental())
    }

    /// Every optional device this implementation supports granted.
    pub fn all_grants(&self) -> Grants {
        self.grants(|_| true)
    }

    /// The first optional device `grants` grants that this implementation
    /// does not support, or `Err(None)` if `grants` does not fit the image
    /// (a flag per optional entry).
    pub fn check_grants(&self, grants: &Grants) -> Result<(), Option<DeviceReq>> {
        self.check_grants_on(grants, false)
    }

    /// [`Image::check_grants`] on a host that may grant experimental
    /// devices (`experimental`): a granted one is accepted if the image's
    /// table is valid; whether a driver serves it is the caller's to check.
    pub fn check_grants_on(&self, grants: &Grants, experimental: bool) -> Result<(), Option<DeviceReq>> {
        if grants.0.len() != self.optional().count() {
            return Err(None);
        }
        let table = experimental && self.experimental().is_some();
        match self.optional().zip(&grants.0).find(|(d, g)| **g && !(d.supported() || (table && d.experimental()))) {
            Some((d, _)) => Err(Some(*d)),
            None => Ok(()),
        }
    }

    /// The devices a machine of this image with `grants` has.
    pub fn device_set(&self, grants: &Grants) -> DeviceSet {
        let mut set = [false; 256];
        set[1] = true;
        let mut g = grants.0.iter();
        for d in &self.devices {
            if !d.optional || g.next().copied().unwrap_or(false) {
                set[d.slot as usize] = true;
            }
        }
        DeviceSet(set)
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
        Image::build("hi", 4, &[DeviceReq::new(2, 0)], &[0x76])
    }

    #[test]
    fn round_trip() {
        let file = valid();
        let img = Image::parse(&file).unwrap();
        assert_eq!(img.title, b"hi");
        assert_eq!(img.ram_pages, 4);
        assert_eq!(img.rom, [0x76]);
        assert!(img.lists(1) && img.lists(2));
        assert!(!img.lists(0) && !img.lists(3));
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
        check(&|f| f[8] = 4, ImageError::RomLength(0x40001));
        check(&|f| f[9] = 0, ImageError::RamPages(0));
        check(&|f| f[9] = 97, ImageError::RamPages(97));
        check(&|f| f[9] = 112, ImageError::RamPages(112));
        check(&|f| f[9] = 128, ImageError::RamPages(128));
        check(&|f| f[10] = 17, ImageError::DeviceCount(17));
        check(&|f| f[15] = 1, ImageError::ReservedNotZero);
        check(&|f| f.push(0), ImageError::LengthMismatch { expected: 35, actual: 36 });
        check(&|f| f[33] = 1, ImageError::UnsupportedDevice(DeviceReq::new(2, 1)));
        check(&|f| f[32] = 5, ImageError::UnsupportedDevice(DeviceReq::new(5, 0)));
        check(&|f| f[32] = 0, ImageError::UnsupportedDevice(DeviceReq::new(0, 0)));
    }

    #[test]
    fn rejects_duplicate_device() {
        let d = DeviceReq::new(2, 0);
        let file = Image::build("", 4, &[d, d], &[0]);
        assert_eq!(Image::parse(&file), Err(ImageError::DuplicateDevice(2)));
    }

    /// SPEC §image: the title is the 16 bytes without their trailing 00h bytes;
    /// a host does not check it (zeros inside, malformed UTF-8 and an
    /// empty title are all accepted). A device slot listed twice is
    /// refused, also slot 1.
    #[test]
    fn titles_are_not_checked() {
        let mut file = valid();
        file[16..32].copy_from_slice(b"a\0\xFF\xFEb\0\0\0\0\0\0\0\0\0\0\0");
        assert_eq!(Image::parse(&file).unwrap().title, b"a\0\xFF\xFEb");
        file[16..32].fill(0);
        assert_eq!(Image::parse(&file).unwrap().title, b"");
        let one = DeviceReq::new(1, 0);
        assert!(Image::parse(&Image::build("", 1, &[one], &[0])).is_ok());
        assert_eq!(Image::parse(&Image::build("", 1, &[one, one], &[0])), Err(ImageError::DuplicateDevice(1)));
    }

    #[test]
    fn one_ram_page_accepted() {
        assert_eq!(Image::parse(&Image::build("", 1, &[], &[0])).unwrap().ram_pages, 1);
    }

    #[test]
    fn max_rom_accepted() {
        assert_eq!((ROM_MAX, RAM_PAGES_MAX), (262_144, 96), "SPEC §profile");
        let file = Image::build("", RAM_PAGES_MAX, &[], &vec![0; ROM_MAX]);
        let image = Image::parse(&file).unwrap();
        assert_eq!((image.rom.len(), image.ram_pages), (ROM_MAX, 96));
    }

    /// SPEC §image: an image of 262,145 bytes of ROM is refused, also when
    /// the file is as long as its header says.
    #[test]
    fn one_byte_over_max_rom_refused() {
        let mut file = Image::build("", 1, &[], &vec![0; ROM_MAX]);
        file.push(0);
        file[6..9].copy_from_slice(&(ROM_MAX as u32 + 1).to_le_bytes()[..3]);
        assert_eq!(Image::parse(&file), Err(ImageError::RomLength(ROM_MAX + 1)));
    }

    /// SPEC §image: the top bit of the version byte marks an optional device;
    /// the version is the other seven bits.
    #[test]
    fn optional_bit() {
        let file = Image::build("", 1, &[DeviceReq::new(2, 0), DeviceReq::optional(4, 0)], &[0]);
        assert_eq!(&file[32..36], &[2, 0x00, 4, 0x80]);
        let img = Image::parse(&file).unwrap();
        assert_eq!(img.devices, [DeviceReq::new(2, 0), DeviceReq::optional(4, 0)]);
        assert!(img.lists(4) && !img.requires(4) && img.requires(2) && img.requires(1));
        assert_eq!(img.optional().count(), 1);
        // An image without the bit is byte for byte what it always was.
        assert_eq!(Image::build("", 1, &[DeviceReq::new(2, 0)], &[0]), valid_one(&[2, 0]));
    }

    fn valid_one(entry: &[u8; 2]) -> Vec<u8> {
        let mut f = Image::build("", 1, &[], &[0]);
        f[10] = 1;
        f.splice(32..32, entry.iter().copied());
        f
    }

    /// Version 127 is the highest; 128 is version 0, optional. A required
    /// device this host lacks (in any version) is refused; an optional one
    /// is accepted and simply not granted.
    #[test]
    fn version_boundary_and_refusal() {
        let p = |e: [u8; 2]| Image::parse(&valid_one(&e));
        assert_eq!(p([2, 127]), Err(ImageError::UnsupportedDevice(DeviceReq::new(2, 127))));
        let img = p([2, 128]).unwrap();
        assert_eq!(img.devices, [DeviceReq::optional(2, 0)]);
        assert_eq!(img.all_grants(), Grants(vec![true]));
        let img = p([2, 255]).unwrap();
        assert_eq!(img.devices, [DeviceReq::optional(2, 127)]);
        assert_eq!(img.all_grants(), Grants(vec![false]));
        assert!(!img.device_set(&img.all_grants()).has(2));
        // Reserved and unknown slots: refused when required, absent when optional.
        assert_eq!(p([5, 0]), Err(ImageError::UnsupportedDevice(DeviceReq::new(5, 0))));
        assert_eq!(p([5, 0x80]).unwrap().all_grants(), Grants(vec![false]));
        assert_eq!(p([200, 0x83]).unwrap().all_grants(), Grants(vec![false]));
        // Device 1 and device 8 cannot be optional.
        assert_eq!(p([1, 0x80]), Err(ImageError::NotOptional(1)));
        assert_eq!(p([8, 0x80]), Err(ImageError::NotOptional(8)));
    }

    /// spec/experimental.md: the table at the end of the ROM names the
    /// experimental slots; a host without drivers never grants them, a
    /// required one is refused.
    #[test]
    fn experimental_devices() {
        let entries = vec![(241, "example.dice".to_string()), (240, "a.b".to_string())];
        let mut rom = vec![0x13, 0, 0, 0];
        rom.extend(experimental_table(&entries));
        let devs = [DeviceReq::new(2, 0), DeviceReq::optional(240, 3), DeviceReq::optional(241, 0)];
        let img = Image::parse(&Image::build("", 1, &devs, &rom)).unwrap();
        assert_eq!(img.experimental(), Some(vec![(240, "a.b".to_string()), (241, "example.dice".to_string())]));
        assert_eq!(img.all_grants(), Grants(vec![false, false]));
        let g = img.grants_on(|_| true, |d, n| d.slot == 241 && n == "example.dice");
        assert_eq!(g, Grants(vec![false, true]));
        assert!(img.grants_experimental(&g) && !img.grants_experimental(&img.all_grants()));
        assert_eq!(img.check_grants(&g), Err(Some(DeviceReq::optional(241, 0))));
        assert_eq!(img.check_grants_on(&g, true), Ok(()));
        // Required: refused by every host.
        let req = [DeviceReq::new(240, 0)];
        assert_eq!(Image::parse(&Image::build("", 1, &req, &rom)), Err(ImageError::UnsupportedDevice(DeviceReq::new(240, 0))));
        // A table that does not match the header grants nothing.
        let one = [DeviceReq::optional(240, 0)];
        let img = Image::parse(&Image::build("", 1, &one, &rom)).unwrap();
        assert_eq!(img.experimental(), None);
        assert_eq!(img.grants_on(|_| true, |_, _| true), Grants(vec![false]));
        let img = Image::parse(&Image::build("", 1, &one, &[0])).unwrap();
        assert_eq!(img.experimental(), None);
        for bad in ["", "Dice", "1dice", "a b", "this-name-is-longer-than-thirty"] {
            assert!(!valid_experimental_name(bad), "{bad}");
        }
        assert!(valid_experimental_name("example.dice") && valid_experimental_name("a-1.x"));
        // No experimental slot: no table looked for.
        assert_eq!(Image::parse(&valid()).unwrap().experimental(), Some(vec![]));
    }

    /// SPEC §image.blobs: the list at the end of the ROM, before an
    /// experimental device table; an invalid one is no list.
    #[test]
    fn blob_lists() {
        let a = ListedBlob { id: [1; 32], size: 30_000 };
        let b = ListedBlob { id: [2; 32], size: 1 };
        let mut rom = vec![0x13, 0, 0, 0];
        rom.extend(blob_list_table(&[a, b]));
        let img = Image::parse(&Image::build("", 1, &[], &rom)).unwrap();
        assert_eq!(img.blob_list(), vec![a, b]);
        // Before the experimental table.
        let mut with_x = rom.clone();
        with_x.extend(experimental_table(&[(240, "a.b".to_string())]));
        let img = Image::parse(&Image::build("", 1, &[DeviceReq::optional(240, 0)], &with_x)).unwrap();
        assert_eq!(img.blob_list(), vec![a, b]);
        assert_eq!(img.experimental(), Some(vec![(240, "a.b".to_string())]));
        // The header lists no experimental slot: the table's bytes are the end.
        assert!(Image::parse(&Image::build("", 1, &[], &with_x)).unwrap().blob_list().is_empty());
        // None, and invalid ones.
        assert!(Image::parse(&valid()).unwrap().blob_list().is_empty());
        let bad = |blobs: &[ListedBlob]| {
            let mut rom = vec![0x13, 0, 0, 0];
            rom.extend(blob_list_table(blobs));
            Image::parse(&Image::build("", 1, &[], &rom)).unwrap().blob_list()
        };
        assert!(bad(&[a, a]).is_empty(), "a blob twice");
        assert!(bad(&[ListedBlob { size: 0, ..a }]).is_empty());
        assert!(bad(&[ListedBlob { size: BLOB_SIZE_MAX + 1, ..a }]).is_empty());
        let big: Vec<ListedBlob> = (0..5).map(|i| ListedBlob { id: [i; 32], size: BLOB_SIZE_MAX }).collect();
        assert!(bad(&big).is_empty(), "over 4 MiB in all");
        assert_eq!(bad(&big[..4]).len(), 4);
        let many: Vec<ListedBlob> = (0..65).map(|i| ListedBlob { id: [i; 32], size: 1 }).collect();
        assert!(check_blob_list(&many).is_err() && check_blob_list(&many[..64]).is_ok());
        let mut short = blob_list_table(&[a]);
        short.remove(0);
        assert!(Image::parse(&Image::build("", 1, &[], &short)).unwrap().blob_list().is_empty());
    }

    /// A slot is listed once, whether required or optional.
    #[test]
    fn duplicate_slots_with_the_optional_bit() {
        let file = Image::build("", 1, &[DeviceReq::new(3, 0), DeviceReq::optional(3, 0)], &[0]);
        assert_eq!(Image::parse(&file), Err(ImageError::DuplicateDevice(3)));
        let file = Image::build("", 1, &[DeviceReq::optional(6, 0), DeviceReq::optional(6, 1)], &[0]);
        assert_eq!(Image::parse(&file), Err(ImageError::DuplicateDevice(6)));
    }

    /// Grants, one per optional entry in header order, and the devices a
    /// machine with them has.
    #[test]
    fn grants_and_device_sets() {
        let devs = [DeviceReq::optional(6, 0), DeviceReq::new(2, 0), DeviceReq::optional(4, 0), DeviceReq::optional(3, 0)];
        let img = Image::parse(&Image::build("", 1, &devs, &[0])).unwrap();
        assert_eq!(img.all_grants(), Grants(vec![true, true, true]));
        let g = img.grants(|d| d.slot != 4);
        assert_eq!(g, Grants(vec![true, false, true]));
        let set = img.device_set(&g);
        assert!(set.has(1) && set.has(2) && set.has(3) && set.has(6) && !set.has(4) && !set.has(7));
        assert_eq!(img.check_grants(&g), Ok(()));
        assert_eq!(img.check_grants(&Grants(vec![true])), Err(None));
        let odd = Image::parse(&Image::build("", 1, &[DeviceReq::optional(5, 0)], &[0])).unwrap();
        assert_eq!(odd.check_grants(&Grants(vec![true])), Err(Some(DeviceReq::optional(5, 0))));
        assert_eq!(odd.check_grants(&Grants(vec![false])), Ok(()));
    }
}
