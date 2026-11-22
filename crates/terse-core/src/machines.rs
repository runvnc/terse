//! Device 8, Machines (SPEC §machines): the shell's handles on the machines it
//! starts. The device holds what the shell has asked for (which image runs
//! under which handle, where it is shown, whether it runs, which machine has
//! the focus) and what the host has latched about each machine for the
//! frame; [`crate::shell::System`] carries the requests out.

use crate::content::{ram_range, readable};
use crate::machine::{ERR_BUFFER, copy_charge};
use crate::page::Region;
use crate::snapshot::{INFO_BYTES as SNAPSHOT_INFO_BYTES, SLOTS, SnapshotInfo, TABLE};
use crate::system::{SCREEN_HEIGHT, SCREEN_WIDTH};

/// The device slot and the version this implementation provides.
pub const SLOT: u8 = 8;
pub const VERSION: u8 = 0;
/// Handles 1 to 8 (SPEC §machines, provisional).
pub const MAX_MACHINES: usize = 8;
/// The record `info` writes.
pub const INFO_BYTES: usize = 48;
/// The record `identity` writes: feed key, name length, name (48, zero
/// padded).
pub const IDENTITY_BYTES: usize = 81;
/// Longest program name (SPEC §machines.calls), as a display name.
pub const MAX_PROGRAM_NAME: usize = 48;
/// An open request as `request` writes it (SPEC §machines.calls): the kind, then a
/// blob ID, or a program identity as `identity` writes it.
pub const REQUEST_BYTES: usize = 82;
/// Open request kinds (SPEC §system `open`).
pub const OPEN_BLOB: u8 = 1;
pub const OPEN_PROGRAM: u8 = 2;

/// Reads the request record of `open` at `addr` (SPEC §system): kind 1 and a
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
/// (SPEC §machines.calls).
pub mod failure {
    pub const NOT_FOUND: u8 = 1;
    pub const INVALID: u8 = 2;
    pub const NEEDS_DEVICE: u8 = 3;
    pub const NO_ROOM: u8 = 4;
    /// `restore`: no snapshot in the slot, or one that cannot be read or
    /// does not fit its image.
    pub const SNAPSHOT: u8 = 5;
}

/// A program's identity (SPEC §machines.shell): the feed that publishes it and its
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

/// Why the image for a handle could not be had (SPEC §machines.calls): the reason
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

/// A machine of `image` cannot have optional device `d` (`None`: grants
/// that do not fit the image), which a restored machine was granted: reason
/// 3 (SPEC §machines.calls `restore`).
pub fn needs_device(image: &crate::image::Image, d: Option<crate::image::DeviceReq>) -> StartFailure {
    StartFailure {
        reason: failure::NEEDS_DEVICE,
        image: Some(image.id),
        message: match d {
            Some(d) => format!("the machine had device {} version {}, which this host does not provide", d.slot, d.version),
            None => "grants that do not fit the image".into(),
        },
    }
}

/// Checks `bytes` as an image a machine can start (SPEC §image): not an image is
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

/// Machine states (SPEC §machines.shell).
pub mod state {
    pub const NONE: u8 = 0;
    pub const LOADING: u8 = 1;
    pub const RUNNING: u8 = 2;
    pub const PAUSED: u8 = 3;
    pub const EXITED: u8 = 4;
    pub const FAULTED: u8 = 5;
    pub const FAILED: u8 = 6;
    /// Stopped by the host: a device it uses did not keep up
    /// (`PERFORMANCE.md` R15).
    pub const HALTED: u8 = 7;

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
            HALTED => "halted",
            _ => "?",
        }
    }
}

/// Call numbers (SPEC §calls.table).
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
    pub const SNAPSHOT: u32 = 0x80B;
    pub const RESTORE: u32 = 0x80C;
    pub const SNAPSHOT_INFO: u32 = 0x80D;
    pub const SNAPSHOT_DELETE: u32 = 0x80E;
    pub const FACE: u32 = 0x80F;
    pub const UNFACE: u32 = 0x810;
    pub const EXPERIMENTAL: u32 = 0x811;
}

/// What `experimental` reports of an entry (SPEC §machines.calls): the
/// image lists the device, and the machine was not granted it, or was.
pub mod experimental {
    pub const LISTED: u8 = 1;
    pub const GRANTED: u8 = 2;
}

/// An entry of the experimental device table as `experimental` writes it,
/// the image's own 32 bytes: slot, name length, name, zeros.
pub const EXPERIMENTAL_BYTES: usize = 32;

/// The experimental devices a machine of `image` with `grants` has, as
/// `experimental` reports them (SPEC §machines.calls): each the image's
/// table lists, in header order, with whether it was granted. None if the
/// table is invalid or missing (then none can be granted anywhere).
pub fn experimental_entries(image: &crate::image::Image, grants: &crate::image::Grants) -> Vec<(u8, [u8; EXPERIMENTAL_BYTES])> {
    let granted: Vec<u8> = image.optional().zip(&grants.0).filter(|(d, g)| **g && d.experimental()).map(|(d, _)| d.slot).collect();
    image
        .experimental()
        .unwrap_or_default()
        .into_iter()
        .map(|(slot, name)| {
            let mut e = [0; EXPERIMENTAL_BYTES];
            e[0] = slot;
            e[1] = name.len() as u8;
            e[2..2 + name.len()].copy_from_slice(name.as_bytes());
            (if granted.contains(&slot) { experimental::GRANTED } else { experimental::LISTED }, e)
        })
        .collect()
}

/// The longest view motion `face` and `unface` take, in frames (SPEC §machines.calls).
pub const MAX_VIEW_FRAMES: u32 = 600;
/// A view's progress runs from 0 (the scene as listed) to this (the panel
/// flat on the glass).
pub const VIEW_DONE: u16 = 4096;
/// The view's bytes in the state (STATE.md 1.10).
pub const VIEW_BYTES: usize = 18;

/// The host's view of the shell's 3D scene (SPEC §machines.calls `face`, STATE.md
/// 1.10): the machine whose panel it moves to the glass (0: none), the
/// rectangle, whether it moves there or back, the progress it started from,
/// the shell's frame counter in the frame of the call and the duration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub handle: u8,
    pub toward: bool,
    pub rect: Region,
    pub from: u16,
    pub start: u32,
    pub duration: u16,
}

impl View {
    /// The progress (0 to [`VIEW_DONE`]) shown after the shell's frame
    /// `frame` (its frame counter in that frame): linear in the frames
    /// since the call, from `from` to the end it moves to.
    pub fn progress(&self, frame: u32) -> u16 {
        let to = if self.toward { VIEW_DONE as i64 } else { 0 };
        let k = frame.wrapping_sub(self.start) as i64;
        if k >= self.duration as i64 {
            return to as u16;
        }
        let from = self.from as i64;
        (from + (to - from) * k / self.duration as i64) as u16
    }

    /// Whether the view still moves after frame `frame`.
    pub fn moving(&self, frame: u32) -> bool {
        self.handle != 0 && self.progress(frame) != if self.toward { VIEW_DONE } else { 0 }
    }

    /// The 18 bytes of STATE.md 1.10: handle, direction, rectangle,
    /// progress at the start, start frame, duration.
    pub fn bytes(&self) -> [u8; VIEW_BYTES] {
        let mut b = [0; VIEW_BYTES];
        b[0] = self.handle;
        b[1] = self.toward as u8;
        for (i, v) in [self.rect.x, self.rect.y, self.rect.w, self.rect.h, self.from].iter().enumerate() {
            b[2 + 2 * i..4 + 2 * i].copy_from_slice(&v.to_le_bytes());
        }
        b[12..16].copy_from_slice(&self.start.to_le_bytes());
        b[16..18].copy_from_slice(&self.duration.to_le_bytes());
        b
    }
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
    /// The machine's experimental devices as `experimental` reports them
    /// ([`experimental_entries`]), set when the machine is made: host data,
    /// an input for the shell, not part of device 8's state.
    pub experimental: Vec<(u8, [u8; EXPERIMENTAL_BYTES])>,
}

impl Slot {
    /// The 48-byte record of `info` (SPEC §machines.calls).
    pub fn info(&self) -> [u8; INFO_BYTES] {
        let mut out = [0; INFO_BYTES];
        out[..4].copy_from_slice(&[self.state, self.exit_code, self.fault_kind, self.failure]);
        out[4..8].copy_from_slice(&self.units.to_le_bytes());
        out[8..12].copy_from_slice(&self.frames.to_le_bytes());
        out[12..16].copy_from_slice(&self.fault_pc.to_le_bytes());
        out[16..].copy_from_slice(&self.image);
        out
    }

    /// Whether the handle is being restored (SPEC §machines.calls `argument`): loading,
    /// with image ID zero and no identity. A `start` of the all-zero image
    /// ID, which can never be found, looks the same.
    pub fn restoring(&self) -> bool {
        self.state == state::LOADING && self.image == [0; 32] && self.identity.is_none()
    }

    /// Whether the machine has a picture to show: it has been loaded.
    pub fn has_picture(&self) -> bool {
        matches!(self.state, state::RUNNING | state::PAUSED | state::EXITED | state::FAULTED | state::HALTED)
    }
}

/// Where a machine's 640 × 480 picture lies inside its rectangle: scaled
/// uniformly to fit and centred (SPEC §machines.screens). `None` if nothing is shown.
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
/// lies on the picture (SPEC §machines.input).
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
    /// `argument`: the blob ID the machine is given (SPEC §machines.calls).
    Argument { handle: u8, id: [u8; 32] },
    /// `restore`: a new machine from the snapshot in `slot`.
    Restore { handle: u8, slot: u8 },
    /// `snapshot`: carried out at the end of the system frame.
    Snapshot { handle: u8, slot: u8 },
    /// `snapshot_delete`: the host removes the slot's snapshot (at the end
    /// of the frame, in order with the snapshots).
    DeleteSnapshot { slot: u8 },
}

/// Device 8's state (STATE.md 1.10) and the shell's requests.
#[derive(Clone, Debug, Default)]
pub struct Machines {
    slots: [Slot; MAX_MACHINES],
    focus: u8,
    builtins: Vec<[u8; 32]>,
    /// The open requests the machines made that the shell has been told of
    /// and not yet read (SPEC §machines.calls `request`), by handle.
    requests: [Option<[u8; REQUEST_BYTES]>; MAX_MACHINES],
    /// Starts and stops since the host last took them (not machine state).
    commands: Vec<Command>,
    /// The shell's snapshot slots as the host has them (SPEC §machines.snapshots): host
    /// data, an input; not machine state.
    snapshots: [Option<SnapshotInfo>; TABLE],
    /// The host's view of the shell's 3D scene (`face`, `unface`).
    view: View,
}

/// What a call of device 8 may look at besides RAM and ROM: the shell's
/// frame counter in this frame and its 3D list (`face`).
#[derive(Clone, Copy, Debug, Default)]
pub struct CallContext<'a> {
    pub frame: u32,
    pub list: &'a [u8],
}

impl Machines {
    pub fn new(builtins: Vec<[u8; 32]>) -> Self {
        Machines { builtins, ..Default::default() }
    }

    /// Carries out call `n` (800h-8FFh); returns (result, units charged
    /// beyond the call itself).
    pub fn call(&mut self, n: u32, a: [u32; 6], rom: &[u8], ram: &mut [u8], ctx: CallContext) -> (u32, u32) {
        let [a0, a1, a2, a3, a4, a5] = a;
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
                // a program name is a bad buffer (SPEC §machines.calls).
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
                // Only a machine that will be reset takes an argument: not
                // one being restored (SPEC §machines.calls), which its record shows as
                // loading with no image ID and no identity.
                match slot(a0).filter(|&i| self.slots[i].state == state::LOADING && !self.slots[i].restoring()) {
                    None => (0, copy_charge(32)),
                    Some(_) => {
                        self.commands.push(Command::Argument { handle: a0 as u8, id });
                        (1, copy_charge(32))
                    }
                }
            }
            call::SNAPSHOT => {
                let ok = slot(a0).is_some_and(|i| matches!(self.slots[i].state, state::RUNNING | state::PAUSED))
                    && (a1 as usize) < SLOTS;
                if !ok {
                    return (0, 0);
                }
                self.commands.push(Command::Snapshot { handle: a0 as u8, slot: a1 as u8 });
                (1, 0)
            }
            call::RESTORE => {
                if a0 as usize >= TABLE {
                    return (0, 0);
                }
                let Some(i) = self.slots.iter().position(|s| s.state == state::NONE) else { return (0, 0) };
                self.slots[i] = Slot { state: state::LOADING, run: true, ..Slot::default() };
                self.requests[i] = None;
                let handle = i as u8 + 1;
                self.commands.push(Command::Restore { handle, slot: a0 as u8 });
                (handle as u32, 0)
            }
            call::SNAPSHOT_INFO => {
                let Some(at) = ram_range(ram.len(), a1, SNAPSHOT_INFO_BYTES) else { return (ERR_BUFFER, 0) };
                if a0 as usize >= TABLE {
                    return (0, 0);
                }
                let record = self.snapshots[a0 as usize].as_ref().map_or([0; SNAPSHOT_INFO_BYTES], SnapshotInfo::record);
                ram[at..at + SNAPSHOT_INFO_BYTES].copy_from_slice(&record);
                (record[0] as u32, copy_charge(SNAPSHOT_INFO_BYTES))
            }
            call::SNAPSHOT_DELETE => {
                if a0 as usize >= TABLE {
                    return (0, 0);
                }
                self.snapshots[a0 as usize] = None;
                self.commands.push(Command::DeleteSnapshot { slot: a0 as u8 });
                (1, 0)
            }
            call::FACE => {
                let mut page = crate::page::Page::default();
                page.set_region(a2, a3, a4, a5);
                let rect = page.region();
                let ok = slot(a0).is_some_and(|i| self.shows_panel(i as u8 + 1, ctx.list))
                    && a1 <= MAX_VIEW_FRAMES
                    && picture(rect).is_some();
                if !ok {
                    return (0, 0);
                }
                // From where the view is now if it moves this machine's
                // panel; else from the scene as listed.
                let from = if self.view.handle == a0 as u8 { self.view.progress(ctx.frame) } else { 0 };
                self.view = View { handle: a0 as u8, toward: true, rect, from, start: ctx.frame, duration: a1 as u16 };
                (1, 0)
            }
            call::UNFACE => {
                if a0 > MAX_VIEW_FRAMES || self.view.handle == 0 {
                    return (0, 0);
                }
                let from = self.view.progress(ctx.frame);
                self.view = View { toward: false, from, start: ctx.frame, duration: a0 as u16, ..self.view };
                (1, 0)
            }
            call::EXPERIMENTAL => {
                let Some(at) = ram_range(ram.len(), a2, EXPERIMENTAL_BYTES) else { return (ERR_BUFFER, 0) };
                match slot(a0).and_then(|i| self.slots[i].experimental.get(a1 as usize)) {
                    None => (0, 0),
                    Some((status, entry)) => {
                        ram[at..at + EXPERIMENTAL_BYTES].copy_from_slice(entry);
                        (*status as u32, copy_charge(EXPERIMENTAL_BYTES))
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

    /// Whether handle `h`'s machine may be shown as a panel (SPEC §vector3d.list
    /// Screen): it has a picture, its rectangle shows none, and `list` has a
    /// Screen of it.
    pub fn shows_panel(&self, h: u8, list: &[u8]) -> bool {
        let s = &self.slots[h as usize - 1];
        s.has_picture() && picture(s.rect).is_none() && crate::vector3::screens_in(list) & 1 << (h - 1) != 0
    }

    /// The handles whose Screens are in effect with the shell's `list`
    /// (bit h − 1 for handle h): as [`Machines::shows_panel`], with the
    /// machine there (`present`, the host's knowledge).
    pub fn panel_mask(&self, list: &[u8], present: impl Fn(u8) -> bool) -> u8 {
        let listed = crate::vector3::screens_in(list);
        (1..=MAX_MACHINES as u8)
            .filter(|&h| {
                let s = &self.slots[h as usize - 1];
                listed & 1 << (h - 1) != 0 && s.has_picture() && picture(s.rect).is_none() && present(h)
            })
            .fold(0, |m, h| m | 1 << (h - 1))
    }

    /// The view (SPEC §machines.calls `face`).
    pub fn view(&self) -> &View {
        &self.view
    }

    /// At the end of the shell's frame: a view whose machine is no longer
    /// shown as a panel with `list` returns at once (SPEC §machines.calls `face`).
    pub fn check_view(&mut self, list: &[u8]) {
        if self.view.handle != 0 && !self.shows_panel(self.view.handle, list) {
            self.view = View::default();
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

    /// Handle `h`'s record, for the host to latch into (SPEC §machines.frames).
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

    /// The home key (SPEC §machines.input): the focus returns to the shell.
    pub fn home(&mut self) {
        self.focus = 0;
    }

    pub fn builtins(&self) -> &[[u8; 32]] {
        &self.builtins
    }

    /// Machine `h`'s open request, for the shell to read (SPEC §machines.frames).
    pub fn set_request(&mut self, h: u8, request: [u8; REQUEST_BYTES]) {
        self.requests[h as usize - 1] = Some(request);
    }

    /// The shell's snapshot slots as the host has them (SPEC §machines.snapshots).
    pub fn snapshots(&self) -> &[Option<SnapshotInfo>; TABLE] {
        &self.snapshots
    }

    pub fn set_snapshot(&mut self, slot: u8, info: Option<SnapshotInfo>) {
        self.snapshots[slot as usize] = info;
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
        out.extend_from_slice(&self.view.bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_calls() {
        let base = crate::cpu::RAM_BASE;
        let mut ram = vec![0u8; 8192];
        let mut m = Machines::new(Vec::new());
        let call = |m: &mut Machines, n, a: [u32; 5], ram: &mut Vec<u8>| m.call(n, [a[0], a[1], a[2], a[3], a[4], 0], &[], ram, CallContext::default()).0;
        assert_eq!(call(&mut m, call::SNAPSHOT, [1, 0, 0, 0, 0], &mut ram), 0, "no machine");
        assert_eq!(call(&mut m, call::RESTORE, [24, 0, 0, 0, 0], &mut ram), 0, "no slot 24 (16-23 are the host's automatic slots)");
        assert_eq!(call(&mut m, call::RESTORE, [3, 0, 0, 0, 0], &mut ram), 1);
        assert_eq!((m.slot(1).state, m.slot(1).run, m.slot(1).image), (state::LOADING, true, [0; 32]));
        assert_eq!(call(&mut m, call::SNAPSHOT, [1, 0, 0, 0, 0], &mut ram), 0, "a loading machine");
        m.slot_mut(1).state = state::PAUSED;
        assert_eq!(call(&mut m, call::SNAPSHOT, [1, 15, 0, 0, 0], &mut ram), 1);
        assert_eq!(call(&mut m, call::SNAPSHOT, [1, 16, 0, 0, 0], &mut ram), 0); // 16-23: the host's
        // Info: an empty slot, a full one, out of range, a bad buffer.
        ram[..122].fill(0xEE);
        assert_eq!(call(&mut m, call::SNAPSHOT_INFO, [4, base, 0, 0, 0], &mut ram), 0);
        assert_eq!(ram[..122], [0; 122]);
        let info = SnapshotInfo { image: [7; 32], identity: None, frames: 99, time: 5 };
        m.set_snapshot(4, Some(info.clone()));
        assert_eq!(call(&mut m, call::SNAPSHOT_INFO, [4, base, 0, 0, 0], &mut ram), 1);
        assert_eq!(ram[..122], info.record());
        assert_eq!((ram[0], &ram[1..33], &ram[114..122]), (1, &[7; 32][..], &[99, 0, 0, 0, 5, 0, 0, 0][..]));
        assert_eq!(call(&mut m, call::SNAPSHOT_INFO, [24, base, 0, 0, 0], &mut ram), 0);
        assert_eq!(call(&mut m, call::SNAPSHOT_INFO, [4, base + 8190, 0, 0, 0], &mut ram), crate::machine::ERR_BUFFER);
        assert_eq!(call(&mut m, call::SNAPSHOT_DELETE, [4, 0, 0, 0, 0], &mut ram), 1);
        assert_eq!(call(&mut m, call::SNAPSHOT_INFO, [4, base, 0, 0, 0], &mut ram), 0, "empty at once");
        assert_eq!(call(&mut m, call::SNAPSHOT_DELETE, [24, 0, 0, 0, 0], &mut ram), 0);
        assert_eq!(
            m.take_commands(),
            [Command::Restore { handle: 1, slot: 3 }, Command::Snapshot { handle: 1, slot: 15 }, Command::DeleteSnapshot { slot: 4 }]
        );
    }

    /// SPEC §machines.calls `experimental`: the image's table entries in
    /// header order, 1 listed or 2 granted; 0 past the last, for a handle
    /// with no machine and out of range; the buffer checked first; none for
    /// an image without a valid table.
    #[test]
    fn experimental_calls() {
        use crate::image::{DeviceReq, Grants, Image, experimental_table};
        let base = crate::cpu::RAM_BASE;
        let mut ram = vec![0u8; 8192];
        let entries = vec![(241, "example.dice".to_string()), (240, "a.b".to_string())];
        let mut rom = vec![0x13, 0, 0, 0];
        rom.extend(experimental_table(&entries));
        let devs = [DeviceReq::new(2, 0), DeviceReq::optional(240, 3), DeviceReq::optional(241, 0)];
        let img = Image::parse(&Image::build("", 1, &devs, &rom)).unwrap();
        let mut m = Machines::new(Vec::new());
        m.slot_mut(2).state = state::RUNNING;
        m.slot_mut(2).experimental = experimental_entries(&img, &Grants(vec![false, true]));
        let mut call = |m: &mut Machines, a0, a1, a2| m.call(call::EXPERIMENTAL, [a0, a1, a2, 0, 0, 0], &[], &mut ram, CallContext::default());
        assert_eq!(call(&mut m, 2, 0, base), (1, 8), "a.b, listed");
        assert_eq!(call(&mut m, 2, 1, base), (2, 8), "example.dice, granted");
        assert_eq!(call(&mut m, 2, 2, base), (0, 0));
        assert_eq!(call(&mut m, 1, 0, base), (0, 0), "no machine");
        assert_eq!(call(&mut m, 9, 0, base), (0, 0));
        assert_eq!(call(&mut m, 2, 0, base + 8170), (crate::machine::ERR_BUFFER, 0));
        let mut want = [0u8; 32];
        want[..14].copy_from_slice(b"\xF1\x0Cexample.dice");
        m.call(call::EXPERIMENTAL, [2, 1, base, 0, 0, 0], &[], &mut ram, CallContext::default());
        assert_eq!(ram[..32], want);
        // The same entries whatever was granted; none without a valid table.
        assert_eq!(experimental_entries(&img, &img.all_grants()).iter().map(|e| e.0).collect::<Vec<_>>(), [1, 1]);
        let plain = Image::parse(&Image::build("", 1, &devs, &[0x13, 0, 0, 0])).unwrap();
        assert!(experimental_entries(&plain, &plain.all_grants()).is_empty());
        // Not machine state: the device's state is the same with and without.
        let (mut a, mut b) = (Vec::new(), Vec::new());
        m.write_state(&mut a);
        m.slot_mut(2).experimental.clear();
        m.write_state(&mut b);
        assert_eq!(a, b);
    }

    /// SPEC §machines.calls `argument`: taken by a handle that is loading after `start`
    /// or `start_program`; refused (0, nothing changes, the ID still read
    /// and charged) for a handle being restored, one not loading and one
    /// out of range; the buffer is checked first.
    #[test]
    fn argument_only_for_a_machine_that_will_be_reset() {
        let base = crate::cpu::RAM_BASE;
        let mut ram = vec![0u8; 8192];
        ram[..32].copy_from_slice(&[9; 32]);
        ram[32] = 1;
        ram[33] = b'p';
        let mut m = Machines::new(Vec::new());
        let mut call = |m: &mut Machines, n, a0, a1| m.call(n, [a0, a1, 0, 0, 0, 0], &[], &mut ram, CallContext::default());
        assert_eq!(call(&mut m, call::START, base, 0), (1, 8));
        assert_eq!(call(&mut m, call::START_PROGRAM, base, 0), (2, 9));
        assert_eq!(call(&mut m, call::RESTORE, 0, 0), (3, 0));
        assert!(!m.slot(1).restoring() && !m.slot(2).restoring() && m.slot(3).restoring());
        m.take_commands();
        assert_eq!(call(&mut m, call::ARGUMENT, 1, base), (1, 8));
        assert_eq!(call(&mut m, call::ARGUMENT, 2, base), (1, 8));
        assert_eq!(call(&mut m, call::ARGUMENT, 3, base), (0, 8), "being restored");
        assert_eq!(call(&mut m, call::ARGUMENT, 4, base), (0, 8), "not in use");
        assert_eq!(call(&mut m, call::ARGUMENT, 9, base), (0, 8));
        assert_eq!(call(&mut m, call::ARGUMENT, 3, base + 8180), (crate::machine::ERR_BUFFER, 0), "buffer first");
        let args: Vec<u8> = m.take_commands().iter().filter_map(|c| if let Command::Argument { handle, .. } = c { Some(*handle) } else { None }).collect();
        assert_eq!(args, [1, 2]);
        m.slot_mut(1).state = state::RUNNING;
        assert_eq!(call(&mut m, call::ARGUMENT, 1, base), (0, 8), "no longer loading");
    }

    /// `face` and `unface` (SPEC §machines.calls): the checks, the progress counted
    /// in the shell's frames, reversing part way, and the view's return when
    /// its machine is no longer a panel.
    #[test]
    fn face_and_unface() {
        let mut ram = vec![0u8; 64];
        let screen = [crate::vector3::op::SCREEN, 1, 0x00, 0x19];
        let mut m = Machines::new(Vec::new());
        let mut call = |m: &mut Machines, n, a: [u32; 6], frame: u32, list: &[u8]| m.call(n, a, &[], &mut ram, CallContext { frame, list }).0;
        let area = |h, d| [h, d, 3, 23, 634, 454];
        assert_eq!(call(&mut m, call::FACE, area(1, 20), 100, &screen), 0, "handle 1 not in use");
        m.slot_mut(1).state = state::LOADING;
        assert_eq!(call(&mut m, call::FACE, area(1, 20), 100, &screen), 0, "no picture yet");
        m.slot_mut(1).state = state::RUNNING;
        assert_eq!(call(&mut m, call::FACE, area(1, 20), 100, &[]), 0, "no Screen in the list");
        assert_eq!(call(&mut m, call::FACE, area(1, 601), 100, &screen), 0, "too long");
        assert_eq!(call(&mut m, call::FACE, [1, 20, 3, 23, 0, 454], 100, &screen), 0, "no picture in the rectangle");
        assert_eq!(call(&mut m, call::FACE, area(9, 20), 100, &screen), 0);
        assert_eq!(call(&mut m, call::UNFACE, [20, 0, 0, 0, 0, 0], 100, &screen), 0, "no view");
        m.slot_mut(1).rect = Region { x: 0, y: 0, w: 64, h: 48 };
        assert_eq!(call(&mut m, call::FACE, area(1, 20), 100, &screen), 0, "placed flat: its Screen is ignored");
        m.slot_mut(1).rect = Region::default();
        assert_eq!(*m.view(), View::default());
        assert_eq!(call(&mut m, call::FACE, area(1, 20), 100, &screen), 1);
        let v = *m.view();
        assert_eq!(v, View { handle: 1, toward: true, rect: Region { x: 3, y: 23, w: 634, h: 454 }, from: 0, start: 100, duration: 20 });
        let p: Vec<u16> = [100, 101, 110, 119, 120, 500].iter().map(|&f| v.progress(f)).collect();
        assert_eq!(p, [0, 204, 2048, 3891, 4096, 4096]);
        assert!(v.moving(119) && !v.moving(120));
        // Back from part way: from where it is.
        assert_eq!(call(&mut m, call::UNFACE, [601, 0, 0, 0, 0, 0], 110, &screen), 0);
        assert_eq!(call(&mut m, call::UNFACE, [10, 0, 0, 0, 0, 0], 110, &screen), 1);
        let v = *m.view();
        assert_eq!((v.toward, v.from, v.start, v.duration), (false, 2048, 110, 10));
        assert_eq!((v.progress(115), v.progress(120)), (1024, 0));
        assert!(v.moving(119) && !v.moving(120));
        // Towards it again, from where it is; no frames: at once.
        assert_eq!(call(&mut m, call::FACE, area(1, 10), 112, &screen), 1);
        assert_eq!(m.view().from, 1639);
        assert_eq!(call(&mut m, call::FACE, area(1, 0), 200, &screen), 1);
        assert_eq!((m.view().progress(200), m.view().moving(200)), (4096, false));
        // The state's bytes.
        let mut out = Vec::new();
        m.write_state(&mut out);
        assert_eq!(out[out.len() - VIEW_BYTES..], [1, 1, 3, 0, 23, 0, 122, 2, 198, 1, 0, 16, 200, 0, 0, 0, 0, 0]);
        // Its Screen leaves the list: the view returns at once.
        m.check_view(&screen);
        assert_eq!(m.view().handle, 1);
        m.check_view(&[]);
        assert_eq!(*m.view(), View::default());
        // Its machine stopped: the view returns at once too.
        assert_eq!(call(&mut m, call::FACE, area(1, 20), 300, &screen), 1);
        assert_eq!(call(&mut m, call::STOP, [1, 0, 0, 0, 0, 0], 300, &screen), 1);
        m.check_view(&screen);
        assert_eq!(*m.view(), View::default());
        let mut none = Vec::new();
        m.write_state(&mut none);
        assert_eq!(none[none.len() - VIEW_BYTES..], [0; VIEW_BYTES], "none: all zero");
    }

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
