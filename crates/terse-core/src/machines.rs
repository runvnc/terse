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
}

/// One handle's record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
    pub image: [u8; 32],
}

impl Slot {
    /// The 48-byte record of `info` (SPEC 12.2).
    pub fn info(&self) -> [u8; INFO_BYTES] {
        let mut out = [0; INFO_BYTES];
        out[..4].copy_from_slice(&[self.state, self.exit_code, self.fault_kind, 0]);
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Start { handle: u8, image: [u8; 32] },
    Stop { handle: u8 },
}

/// Device 8's state (STATE.md 1.10) and the shell's requests.
#[derive(Clone, Debug, Default)]
pub struct Machines {
    slots: [Slot; MAX_MACHINES],
    focus: u8,
    builtins: Vec<[u8; 32]>,
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
                let handle = i as u8 + 1;
                self.commands.push(Command::Start { handle, image: id });
                (handle as u32, copy_charge(32))
            }
            call::STOP => match slot(a0).filter(|&i| self.slots[i].state != state::NONE) {
                None => (0, 0),
                Some(i) => {
                    self.slots[i] = Slot::default();
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

    /// STATE.md 1.10, after the present flag.
    pub(crate) fn write_state(&self, out: &mut Vec<u8>) {
        out.push(self.focus);
        for s in &self.slots {
            out.extend_from_slice(&[s.state, s.run as u8]);
            for v in [s.rect.x, s.rect.y, s.rect.w, s.rect.h] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&s.info()[1..]);
        }
        out.push(self.builtins.len() as u8);
        for id in &self.builtins {
            out.extend_from_slice(id);
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
