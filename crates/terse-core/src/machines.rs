//! Device 8, Machines (SPEC 12): the shell's handles on the machines it
//! starts. The device holds what the shell has asked for (which image runs
//! under which handle, where it is shown, whether it runs, which machine has
//! the focus) and what the host has latched about each machine for the
//! frame; [`crate::shell::System`] carries the requests out.

use crate::content::{ram_range, readable};
use crate::machine::{ERR_BUFFER, copy_charge};
use crate::page::Region;
use crate::system::{SCREEN_HEIGHT, SCREEN_WIDTH};

/// The device slot and the version this implementation provides.
pub const SLOT: u8 = 8;
pub const VERSION: u8 = 0;
/// Handles 1 to 8 (SPEC 12, provisional).
pub const MAX_MACHINES: usize = 8;
/// The record `info` writes.
pub const INFO_BYTES: usize = 48;
/// The record `identity` writes: feed key, name length, name (48, zero
/// padded).
pub const IDENTITY_BYTES: usize = 81;
/// Longest program name (SPEC 12.2), as a display name.
pub const MAX_PROGRAM_NAME: usize = 48;
/// An open request as `request` writes it (SPEC 12.2): the kind, then a
/// blob ID, or a program identity as `identity` writes it.
pub const REQUEST_BYTES: usize = 82;
/// Open request kinds (SPEC 4 `open`).
pub const OPEN_BLOB: u8 = 1;
pub const OPEN_PROGRAM: u8 = 2;

/// Reads the request record of `open` at `addr` (SPEC 4): kind 1 and a
/// blob ID, or kind 2, a feed key, the name's length and a valid program
/// name; all wholly in ROM or wholly in RAM. Returns the request as
/// `request` writes it and the number of bytes read, or `None` for a bad
/// buffer.
pub fn read_open_request(rom: &[u8], ram: &[u8], addr: u32) -> Option<([u8; REQUEST_BYTES], usize)> {
    let mut out = [0; REQUEST_BYTES];
    match readable(rom, ram, addr, 1)?[0] {
        OPEN_BLOB => {
            out[..33].copy_from_slice(readable(rom, ram, addr, 33)?);
            Some((out, 33))
        }
        OPEN_PROGRAM => {
            let n = readable(rom, ram, addr, 34)?[33] as usize;
            let record = readable(rom, ram, addr, 34 + n)?;
            if n > MAX_PROGRAM_NAME || !crate::content::valid_display_name(&record[34..]) {
                return None;
            }
            out[..34 + n].copy_from_slice(record);
            Some((out, 34 + n))
        }
        _ => None,
    }
}

/// Why a start failed: byte 3 of the `info` record of a failed handle
/// (SPEC 12.2).
pub mod failure {
    pub const NOT_FOUND: u8 = 1;
    pub const INVALID: u8 = 2;
    pub const NEEDS_DEVICE: u8 = 3;
    pub const NO_ROOM: u8 = 4;
}

/// A program's identity (SPEC 12.2): the feed that publishes it and its
/// name. Built-in programs have the all-zero key and their image title.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProgramId {
    pub publisher: [u8; 32],
    pub name: Vec<u8>,
}

impl ProgramId {
    /// The 81-byte record of `identity`.
    pub fn record(&self) -> [u8; IDENTITY_BYTES] {
        let mut out = [0; IDENTITY_BYTES];
        out[..32].copy_from_slice(&self.publisher);
        out[32] = self.name.len() as u8;
        out[33..33 + self.name.len()].copy_from_slice(&self.name);
        out
    }

    /// Whether this is a built-in program (the all-zero key).
    pub fn is_builtin(&self) -> bool {
        self.publisher == [0; 32]
    }
}

/// Why the image for a handle could not be had (SPEC 12.2): the reason
/// `info` reports, the image ID if the host got as far as a blob, and a
/// message for people.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartFailure {
    pub reason: u8,
    pub image: Option<[u8; 32]>,
    pub message: String,
}

impl StartFailure {
    pub fn not_found(message: impl Into<String>) -> Self {
        StartFailure { reason: failure::NOT_FOUND, image: None, message: message.into() }
    }
}

/// Checks `bytes` as an image a machine can start (SPEC 2): not an image is
/// reason 2, an image that needs a device (slot, version) this host does
/// not provide reason 3.
pub fn load_image(bytes: &[u8]) -> Result<crate::image::Image, StartFailure> {
    crate::image::Image::parse(bytes).map_err(|e| StartFailure {
        reason: if matches!(e, crate::image::ImageError::UnsupportedDevice(_)) {
            failure::NEEDS_DEVICE
        } else {
            failure::INVALID
        },
        image: Some(*blake3::hash(bytes).as_bytes()),
        message: e.to_string(),
    })
}

/// Machine states (SPEC 12.2).
pub mod state {
    pub const NONE: u8 = 0;
    pub const LOADING: u8 = 1;
    pub const RUNNING: u8 = 2;
    pub const PAUSED: u8 = 3;
    pub const EXITED: u8 = 4;
    pub const FAULTED: u8 = 5;
    pub const FAILED: u8 = 6;

    /// The state's name, as dumps show it.
    pub fn name(s: u8) -> &'static str {
        match s {
            NONE => "none",
            LOADING => "loading",
            RUNNING => "running",
            PAUSED => "paused",
            EXITED => "exited",
            FAULTED => "faulted",
            FAILED => "failed",
            _ => "?",
        }
    }
}

/// Call numbers (SPEC 3.2).
pub mod call {
    pub const START: u32 = 0x800;
    pub const STOP: u32 = 0x801;
    pub const PLACE: u32 = 0x802;
    pub const RUN: u32 = 0x803;
    pub const FOCUS: u32 = 0x804;
    pub const INFO: u32 = 0x805;
    pub const BUILTIN: u32 = 0x806;
    pub const START_PROGRAM: u32 = 0x807;
    pub const IDENTITY: u32 = 0x808;
    pub const REQUEST: u32 = 0x809;
    pub const ARGUMENT: u32 = 0x80A;
}

/// One handle's record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Slot {
    pub state: u8,
    /// The run flag: set by `start` and `run`.
    pub run: bool,
    /// Where the machine's screen is shown, in the shell's screen units.
    pub rect: Region,
    pub exit_code: u8,
    /// The fault kind as STATE.md numbers it (1 illegal … 6 misaligned).
    pub fault_kind: u8,
    /// Units used in the machine's last frame.
    pub units: u32,
    /// The machine's frame counter.
    pub frames: u32,
    pub fault_pc: u32,
    /// Why the start failed, if the state is failed (`failure`), else 0.
    pub failure: u8,
    pub image: [u8; 32],
    /// The identity the machine was started with (`start_program`).
    pub identity: Option<ProgramId>,
}

impl Slot {
    /// The 48-byte record of `info` (SPEC 12.2).
    pub fn info(&self) -> [u8; INFO_BYTES] {
        let mut out = [0; INFO_BYTES];
        out[..4].copy_from_slice(&[self.state, self.exit_code, self.fault_kind, self.failure]);
        out[4..8].copy_from_slice(&self.units.to_le_bytes());
        out[8..12].copy_from_slice(&self.frames.to_le_bytes());
        out[12..16].copy_from_slice(&self.fault_pc.to_le_bytes());
        out[16..].copy_from_slice(&self.image);
        out
    }

    /// Whether the machine has a picture to show: it has been loaded.
    pub fn has_picture(&self) -> bool {
        matches!(self.state, state::RUNNING | state::PAUSED | state::EXITED | state::FAULTED)
    }
}

/// Where a machine's 640 × 480 picture lies inside its rectangle: scaled
/// uniformly to fit and centred (SPEC 12.3). `None` if nothing is shown.
pub fn picture(rect: Region) -> Option<Region> {
    let (w, h) = (rect.w as u32, rect.h as u32);
    let (sw, sh) = (SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32);
    let (pw, ph) = if w * sh <= h * sw { (w, w * sh / sw) } else { (h * sw / sh, h) };
    if pw == 0 || ph == 0 {
        return None;
    }
    let x = rect.x as u32 + (w - pw) / 2;
    let y = rect.y as u32 + (h - ph) / 2;
    Some(Region { x: x as u16, y: y as u16, w: pw as u16, h: ph as u16 })
}

/// A position in the shell's units mapped into the machine's screen, if it
/// lies on the picture (SPEC 12.4).
pub fn map_into(picture: Region, x: u16, y: u16) -> Option<(u16, u16)> {
    let (px, py, pw, ph) = (picture.x as u32, picture.y as u32, picture.w as u32, picture.h as u32);
    let (x, y) = (x as u32, y as u32);
    if x < px || y < py || x >= px + pw || y >= py + ph {
        return None;
    }
    // The centre of the shell's unit, scaled.
    let mx = (2 * (x - px) + 1) * SCREEN_WIDTH as u32 / (2 * pw);
    let my = (2 * (y - py) + 1) * SCREEN_HEIGHT as u32 / (2 * ph);
    Some((mx as u16, my as u16))
}

/// What the shell asked for that the host carries out after its frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Start { handle: u8, image: [u8; 32] },
    /// `start_program`: the current version of a published (or built-in)
    /// program.
    StartProgram { handle: u8, program: ProgramId },
    Stop { handle: u8 },
    /// `argument`: the blob ID the machine is given (SPEC 12.2).
    Argument { handle: u8, id: [u8; 32] },
}

/// Device 8's state (STATE.md 1.10) and the shell's requests.
#[derive(Clone, Debug, Default)]
pub struct Machines {
    slots: [Slot; MAX_MACHINES],
    focus: u8,
    builtins: Vec<[u8; 32]>,
    /// The open requests the machines made that the shell has been told of
    /// and not yet read (SPEC 12.2 `request`), by handle.
    requests: [Option<[u8; REQUEST_BYTES]>; MAX_MACHINES],
    /// Starts and stops since the host last took them (not machine state).
    commands: Vec<Command>,
}

impl Machines {
    pub fn new(builtins: Vec<[u8; 32]>) -> Self {
        Machines { builtins, ..Default::default() }
    }

    /// Carries out call `n` (800h-8FFh); returns (result, units charged
    /// beyond the call itself).
    pub fn call(&mut self, n: u32, a: [u32; 5], rom: &[u8], ram: &mut [u8]) -> (u32, u32) {
        let [a0, a1, a2, a3, a4] = a;
        let slot = |h: u32| (1..=MAX_MACHINES as u32).contains(&h).then(|| h as usize - 1);
        match n {
            call::START => {
                let Some(id) = readable(rom, ram, a0, 32) else { return (ERR_BUFFER, 0) };
                let id: [u8; 32] = id.try_into().unwrap();
                let Some(i) = self.slots.iter().position(|s| s.state == state::NONE) else {
                    return (0, copy_charge(32));
                };
                self.slots[i] = Slot { state: state::LOADING, run: true, image: id, ..Slot::default() };
                self.requests[i] = None;
                let handle = i as u8 + 1;
                self.commands.push(Command::Start { handle, image: id });
                (handle as u32, copy_charge(32))
            }
            call::START_PROGRAM => {
                // Key and length first, then the name; a name that is not
                // a program name is a bad buffer (SPEC 12.2).
                let Some(head) = readable(rom, ram, a0, 33) else { return (ERR_BUFFER, 0) };
                let n = head[32] as usize;
                let Some(record) = readable(rom, ram, a0, 33 + n) else { return (ERR_BUFFER, 0) };
                if n > MAX_PROGRAM_NAME || !crate::content::valid_display_name(&record[33..]) {
                    return (ERR_BUFFER, 0);
                }
                let program = ProgramId { publisher: record[..32].try_into().unwrap(), name: record[33..].to_vec() };
                let charge = copy_charge(33 + n);
                let Some(i) = self.slots.iter().position(|s| s.state == state::NONE) else { return (0, charge) };
                self.slots[i] = Slot { state: state::LOADING, run: true, identity: Some(program.clone()), ..Slot::default() };
                self.requests[i] = None;
                let handle = i as u8 + 1;
                self.commands.push(Command::StartProgram { handle, program });
                (handle as u32, charge)
            }
            call::IDENTITY => {
                let Some(at) = ram_range(ram.len(), a1, IDENTITY_BYTES) else { return (ERR_BUFFER, 0) };
                match slot(a0).and_then(|i| self.slots[i].identity.as_ref()) {
                    None => (0, 0),
                    Some(id) => {
                        ram[at..at + IDENTITY_BYTES].copy_from_slice(&id.record());
                        (1, copy_charge(IDENTITY_BYTES))
                    }
                }
            }
            call::STOP => match slot(a0).filter(|&i| self.slots[i].state != state::NONE) {
                None => (0, 0),
                Some(i) => {
                    self.slots[i] = Slot::default();
                    self.requests[i] = None;
                    if self.focus as usize == i + 1 {
                        self.focus = 0;
                    }
                    self.commands.push(Command::Stop { handle: a0 as u8 });
                    (1, 0)
                }
            },
            call::PLACE => match slot(a0).filter(|&i| self.slots[i].state != state::NONE) {
                None => (0, 0),
                Some(i) => {
                    let mut page = crate::page::Page::default();
                    page.set_region(a1, a2, a3, a4);
                    self.slots[i].rect = page.region();
                    (1, 0)
                }
            },
            call::RUN => {
                let ok = slot(a0).filter(|&i| {
                    matches!(self.slots[i].state, state::LOADING | state::RUNNING | state::PAUSED) && a1 <= 1
                });
                match ok {
                    None => (0, 0),
                    Some(i) => {
                        let s = &mut self.slots[i];
                        s.run = a1 == 1;
                        if s.state != state::LOADING {
                            s.state = if s.run { state::RUNNING } else { state::PAUSED };
                        }
                        (1, 0)
                    }
                }
            }
            call::FOCUS => {
                if a0 == 0 {
                    self.focus = 0;
                    return (1, 0);
                }
                match slot(a0).filter(|&i| matches!(self.slots[i].state, state::LOADING | state::RUNNING | state::PAUSED)) {
                    None => (0, 0),
                    Some(_) => {
                        self.focus = a0 as u8;
                        (1, 0)
                    }
                }
            }
            call::INFO => {
                let Some(at) = ram_range(ram.len(), a1, INFO_BYTES) else { return (ERR_BUFFER, 0) };
                match slot(a0) {
                    None => (0, 0),
                    Some(i) => {
                        ram[at..at + INFO_BYTES].copy_from_slice(&self.slots[i].info());
                        (1, copy_charge(INFO_BYTES))
                    }
                }
            }
            call::REQUEST => {
                let Some(at) = ram_range(ram.len(), a1, REQUEST_BYTES) else { return (ERR_BUFFER, 0) };
                match slot(a0).and_then(|i| self.requests[i].take()) {
                    None => (0, 0),
                    Some(request) => {
                        ram[at..at + REQUEST_BYTES].copy_from_slice(&request);
                        (1, copy_charge(REQUEST_BYTES))
                    }
                }
            }
            call::ARGUMENT => {
                let Some(id) = readable(rom, ram, a1, 32) else { return (ERR_BUFFER, 0) };
                let id: [u8; 32] = id.try_into().unwrap();
                match slot(a0).filter(|&i| self.slots[i].state == state::LOADING) {
                    None => (0, copy_charge(32)),
                    Some(_) => {
                        self.commands.push(Command::Argument { handle: a0 as u8, id });
                        (1, copy_charge(32))
                    }
                }
            }
            call::BUILTIN => {
                let Some(at) = ram_range(ram.len(), a1, 32) else { return (ERR_BUFFER, 0) };
                match self.builtins.get(a0 as usize) {
                    None => (0, 0),
                    Some(id) => {
                        ram[at..at + 32].copy_from_slice(id);
                        (1, copy_charge(32))
                    }
                }
            }
            _ => (crate::machine::ERR_UNKNOWN, 0),
        }
    }

    /// The shell's starts and stops since the last call, in order.
    pub fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.commands)
    }

    /// Handle `h`'s record (1-8).
    pub fn slot(&self, h: u8) -> &Slot {
        &self.slots[h as usize - 1]
    }

    /// Handle `h`'s record, for the host to latch into (SPEC 12.5).
    pub fn slot_mut(&mut self, h: u8) -> &mut Slot {
        &mut self.slots[h as usize - 1]
    }

    /// Sets the state of handle `h` as the host observed it (not commanded
    /// by the shell). A machine that can no longer take input loses the
    /// focus.
    pub fn set_state(&mut self, h: u8, state: u8) {
        self.slots[h as usize - 1].state = state;
        if self.focus == h && !matches!(state, state::LOADING | state::RUNNING | state::PAUSED) {
            self.focus = 0;
        }
    }

    pub fn focus(&self) -> u8 {
        self.focus
    }

    /// The home key (SPEC 12.4): the focus returns to the shell.
    pub fn home(&mut self) {
        self.focus = 0;
    }

    pub fn builtins(&self) -> &[[u8; 32]] {
        &self.builtins
    }

    /// Machine `h`'s open request, for the shell to read (SPEC 12.5).
    pub fn set_request(&mut self, h: u8, request: [u8; REQUEST_BYTES]) {
        self.requests[h as usize - 1] = Some(request);
    }

    /// Whether machine `h`'s request still waits for the shell to read it.
    pub fn request_waiting(&self, h: u8) -> bool {
        self.requests[h as usize - 1].is_some()
    }

    /// STATE.md 1.10, after the present flag.
    pub(crate) fn write_state(&self, out: &mut Vec<u8>) {
        out.push(self.focus);
        for s in &self.slots {
            out.extend_from_slice(&[s.state, s.run as u8]);
            for v in [s.rect.x, s.rect.y, s.rect.w, s.rect.h] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&s.info()[1..]);
            out.extend_from_slice(&s.identity.as_ref().map_or([0; IDENTITY_BYTES], ProgramId::record));
        }
        out.push(self.builtins.len() as u8);
        for id in &self.builtins {
            out.extend_from_slice(id);
        }
        out.push(self.requests.iter().flatten().count() as u8);
        for (i, r) in self.requests.iter().enumerate() {
            if let Some(r) = r {
                out.push(i as u8 + 1);
                out.extend_from_slice(r);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_fit_and_centre() {
        let r = |x, y, w, h| Region { x, y, w, h };
        assert_eq!(picture(r(0, 0, 640, 480)), Some(r(0, 0, 640, 480)));
        assert_eq!(picture(r(0, 20, 640, 460)), Some(r(13, 20, 613, 460)));
        assert_eq!(picture(r(0, 0, 320, 480)), Some(r(0, 120, 320, 240)));
        assert_eq!(picture(r(5, 5, 0, 100)), None);
        assert_eq!(picture(r(5, 5, 1, 100)), None, "1 x 0 shows nothing");
        let p = r(13, 20, 613, 460);
        assert_eq!(map_into(p, 13, 20), Some((0, 0)));
        assert_eq!(map_into(p, 625, 479), Some((639, 479)));
        assert_eq!(map_into(p, 12, 30), None);
        assert_eq!(map_into(p, 626, 30), None);
        assert_eq!(map_into(p, 319, 250), Some((320, 240)));
        assert_eq!(map_into(r(0, 0, 320, 240), 1, 1), Some((3, 3)));
    }
}
