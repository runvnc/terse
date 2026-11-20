//! What the machine's frame loop (`machine.rs`) needs of its devices: the
//! [`Devices`] trait, behind which every device but the System device
//! (slot 1) and the Machines device (slot 8) lives, and [`Memory`], the
//! guest's ROM and RAM. [`InProcess`] is the devices compiled in, as `terse
//! run`, terse-host and a system of machines use them; terse-bus implements
//! the same trait over driver processes (DEVICES.md).

use std::borrow::Cow;

use crate::content::{Completion, Content, ram_range, readable};
use crate::machines::{ProgramId, StartFailure};
use crate::snapshot::{Reader, RestoreError, bad};
use crate::machine::{ERR_BUFFER, ERR_UNKNOWN, FrameInput, call, charge, copy_charge};
use crate::page::Page;
use crate::system::{Event, System};
use crate::terminal::{CELL_BYTES, CELLS, COLS, ROWS, Terminal};
use crate::vector::Vector;
use crate::sound::Sound;
use crate::vector3::Vector3;

/// The guest's memory as a device sees it during a call or at a frame's
/// start.
pub struct GuestMem<'a> {
    pub rom: &'a [u8],
    pub ram: &'a mut [u8],
}

/// What a device call did (SPEC 3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallOutcome {
    pub a0: u32,
    pub a1: Option<u32>,
    /// Units beyond the `ecall` instruction and the call's 4: the rest of
    /// an answered call's base charge (SPEC 14) included.
    pub charge: u32,
}

/// Where an event a device gives at a frame's start goes among the host's
/// input events (DEVICES.md 1.4).
pub mod position {
    /// Before the host's events.
    pub const BEFORE: u8 = 0;
    /// After the host's event whose index the event gives.
    pub const AFTER_HOST: u8 = 1;
    /// After the input events.
    pub const AFTER_INPUT: u8 = 2;
    /// A completion's event (SPEC 9.2), queued after the frame's input.
    pub const COMPLETION: u8 = 3;
}

/// Event flag: this event and the next of the same device are queued
/// together or not at all (SPEC 11.5's page located events).
pub const EVENT_PAIR: u8 = 1;

/// An event a device gives at a frame's start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PosEvent {
    pub event: Event,
    pub position: u8,
    pub flags: u8,
    pub index: u16,
}

/// What the devices give at the start of a frame.
#[derive(Clone, Debug, Default)]
pub struct FrameInputs {
    pub events: Vec<PosEvent>,
    /// Units subtracted from the frame's budget (content delivered, SPEC 14).
    pub charge: u32,
    /// Completions that matched no pending request.
    pub completions_ignored: usize,
}

/// The devices other than System (slot 1) and Machines (slot 8), which are
/// the machine's own. The machine keeps its semantics (the budget, the
/// application of charges, the event queue and its rules, the frame loop,
/// the frame of the state serialization); the devices compute results and
/// charges and give their inputs and state sections.
pub trait Devices {
    /// Step 1 of a frame (SPEC 1.4): the devices' inputs. They may write
    /// guest memory (content delivered). `frame` is the frame counter.
    fn begin_frame(&mut self, frame: u32, input: &FrameInput, mem: GuestMem) -> Result<FrameInputs, String>;
    /// A frame of a machine that has stopped: nothing runs.
    fn idle_frame(&mut self) {}
    /// A call to `slot`, which the image lists (not 1 or 8). `budget` is
    /// what is left after the `ecall` instruction and the call's 4.
    fn call(&mut self, slot: u8, function: u8, args: [u32; 6], budget: i64, mem: GuestMem) -> Result<CallOutcome, String>;
    /// Step 3: the frame has ended; `frame` is the counter after it.
    fn end_frame(&mut self, frame: u32, stopped: bool, mem: GuestMem) -> Result<(), String>;
    /// Slot 2, 3, 4, 6, 7 or 9's section of the state serialization,
    /// without a present flag (STATE.md 1.6–1.9, 1.11, 1.12).
    fn write_state(&mut self, slot: u8, out: &mut Vec<u8>) -> Result<(), String>;
    /// Restores slot 2, 3, 4, 6, 7 or 9's section from the start of `bytes`
    /// (the rest of a serialization, STATE.md 4); returns how many bytes
    /// the section has. `ram_len`: the machine's RAM.
    fn restore_state(&mut self, slot: u8, bytes: &[u8], ram_len: usize) -> Result<usize, RestoreError>;
    /// The samples device 6 rendered at the end of the last frame (SPEC
    /// 18.7), left and right interleaved, once: `None` if no frame ended
    /// since the last call. Only asked of a machine whose image lists
    /// device 6.
    fn take_audio(&mut self) -> Option<Vec<i16>> {
        None
    }
    /// Device 9's list (empty without device 9), which device 8 looks at
    /// (SPEC 12.2 `face`) and which a host needs for the shell's scene and
    /// the pointer over panels (SPEC 12.3, 12.4).
    fn list3(&self) -> Cow<'_, [u8]>;
    /// A new machine for `image`, at reset, with devices of the same kind
    /// as these (a shell's child, SPEC 12.1): `serial` names it among the
    /// system's machines; `identity` is the program's (SPEC 12.2), whose
    /// named records it reads.
    fn new_machine(&self, serial: u32, image: &crate::Image, identity: Option<&ProgramId>) -> Result<crate::Machine<Self>, StartFailure>
    where
        Self: Sized;
}

/// Guest memory that lives elsewhere (a file shared with device drivers).
pub trait Backing: Send {
    /// ROM and RAM.
    fn parts(&self) -> (&[u8], &[u8]);
    fn parts_mut(&mut self) -> (&[u8], &mut [u8]);
}

/// The guest's ROM and RAM.
pub enum Memory {
    Owned { rom: Vec<u8>, ram: Vec<u8> },
    Backed(Box<dyn Backing>),
}

impl Memory {
    #[inline(always)]
    pub fn split(&mut self) -> (&[u8], &mut [u8]) {
        match self {
            Memory::Owned { rom, ram } => (rom, ram),
            Memory::Backed(b) => b.parts_mut(),
        }
    }

    pub fn rom(&self) -> &[u8] {
        match self {
            Memory::Owned { rom, .. } => rom,
            Memory::Backed(b) => b.parts().0,
        }
    }

    pub fn ram(&self) -> &[u8] {
        match self {
            Memory::Owned { ram, .. } => ram,
            Memory::Backed(b) => b.parts().1,
        }
    }
}

/// The devices compiled in: terminal, 2D and 3D vector displays, content
/// and page. Content requests are the host's to carry out
/// ([`InProcess::take_content_requests`]); its completions come in the
/// frame's input.
pub struct InProcess {
    pub(crate) terminal: Option<Terminal>,
    /// Device 3; also there (for the style and persistence it shares with
    /// device 9, SPEC 13.2) when the image lists only device 9.
    pub(crate) vector: Option<Vector>,
    pub(crate) vector3: Option<Vector3>,
    pub(crate) content: Option<Content>,
    pub(crate) page: Option<Page>,
    pub(crate) sound: Option<Sound>,
    /// The image lists device 3 (not only device 9).
    lists_vector: bool,
}

impl InProcess {
    pub fn new(image: &crate::Image) -> Self {
        InProcess {
            lists_vector: image.has_device(3),
            terminal: image.has_device(2).then(Terminal::default),
            vector: (image.has_device(3) || image.has_device(crate::vector3::SLOT)).then(Vector::default),
            vector3: image.has_device(crate::vector3::SLOT).then(Vector3::default),
            content: image.has_device(4).then(Content::default),
            page: image.has_device(7).then(Page::default),
            sound: image.has_device(crate::sound::SLOT).then(Sound::default),
        }
    }

    pub fn terminal(&self) -> Option<&Terminal> {
        self.terminal.as_ref()
    }

    pub fn vector(&self) -> Option<&Vector> {
        self.vector.as_ref()
    }

    pub fn vector3(&self) -> Option<&Vector3> {
        self.vector3.as_ref()
    }

    pub fn vector3_mut(&mut self) -> Option<&mut Vector3> {
        self.vector3.as_mut()
    }

    pub fn content(&self) -> Option<&Content> {
        self.content.as_ref()
    }

    pub fn page(&self) -> Option<&Page> {
        self.page.as_ref()
    }

    pub fn sound(&self) -> Option<&Sound> {
        self.sound.as_ref()
    }

    /// Content requests submitted since the last call.
    pub fn take_content_requests(&mut self) -> Vec<crate::Request> {
        self.content.as_mut().map_or_else(Vec::new, Content::take_requests)
    }
}

impl Devices for InProcess {
    fn begin_frame(&mut self, _frame: u32, input: &FrameInput, mem: GuestMem) -> Result<FrameInputs, String> {
        // The last frame's locates are the host's now.
        self.idle_frame();
        let mut out = FrameInputs::default();
        match &mut self.content {
            Some(content) => {
                // After a restore, the requests pending in the snapshot fail
                // first (SPEC 9.2).
                let orphans = content.take_orphans();
                let joined: Vec<Completion>;
                let completions = if orphans.is_empty() {
                    input.completions
                } else {
                    joined = orphans.into_iter().chain(input.completions.iter().cloned()).collect();
                    &joined
                };
                let mut events = System::default();
                let (ignored, _, delivered) = content.complete(completions, mem.ram, &mut events);
                out.completions_ignored = ignored;
                out.charge = copy_charge(delivered);
                out.events.extend(events.queue().map(|&event| PosEvent { event, position: position::COMPLETION, flags: 0, index: 0 }));
            }
            None => out.completions_ignored = input.completions.len(),
        }
        Ok(out)
    }

    fn idle_frame(&mut self) {
        if let Some(page) = &mut self.page {
            page.begin_frame();
        }
    }

    fn call(&mut self, slot: u8, function: u8, args: [u32; 6], _budget: i64, mem: GuestMem) -> Result<CallOutcome, String> {
        let n = (slot as u32) << 8 | function as u32;
        let [a0, a1, a2, a3, ..] = args;
        let (rom, ram) = (mem.rom, mem.ram);
        let mut units = if call::answered(n) { charge::ANSWERED - charge::CALL } else { 0 };
        let mut r1 = None;
        let r0 = match slot {
            2 => {
                let t = self.terminal.as_mut().unwrap();
                match n {
                    // The whole buffer, at most 4,096 bytes (SPEC 8.2).
                    call::TERM_WRITE => match readable(rom, ram, a0, a1 as usize).filter(|_| a1 <= charge::MAX_WRITE) {
                        None => ERR_BUFFER,
                        Some(bytes) => {
                            t.write(bytes);
                            units += a1 * charge::WRITE_BYTE;
                            a1
                        }
                    },
                    call::TERM_COMMAND => {
                        if a0 < 0x100 {
                            units += t.command(a0 as u8) * charge::SCROLL_CLEAR;
                        }
                        0
                    }
                    call::TERM_SET_CURSOR => {
                        t.set_cursor(a0, a1);
                        0
                    }
                    call::TERM_GET_CURSOR => {
                        let (col, row) = t.cursor();
                        r1 = Some(row as u32);
                        col as u32
                    }
                    call::TERM_SET_COLOURS => {
                        // A value above 255 leaves that colour as it is.
                        let (fg, bg) = t.colors();
                        t.set_colours(u8::try_from(a0).unwrap_or(fg), u8::try_from(a1).unwrap_or(bg));
                        0
                    }
                    call::TERM_GET_COLOURS => {
                        let (fg, bg) = t.colors();
                        r1 = Some(bg as u32);
                        fg as u32
                    }
                    call::TERM_CELLS_READ => {
                        // A count over 2,400 is refused before it is multiplied.
                        let len = if a2 as usize <= CELLS { a2 as usize * CELL_BYTES } else { usize::MAX };
                        match ram_range(ram.len(), a1, len).filter(|_| (a0 as usize) < CELLS && a2 as usize <= CELLS) {
                            None => ERR_BUFFER,
                            Some(at) => {
                                t.cells_read(a0 as usize, &mut ram[at..at + len]);
                                units += copy_charge(len);
                                a2
                            }
                        }
                    }
                    call::TERM_CELLS_WRITE => {
                        let len = if a2 as usize <= CELLS { a2 as usize * CELL_BYTES } else { usize::MAX };
                        match readable(rom, ram, a1, len).filter(|_| (a0 as usize) < CELLS && a2 as usize <= CELLS) {
                            None => ERR_BUFFER,
                            Some(bytes) => {
                                t.cells_write(a0 as usize, bytes);
                                units += copy_charge(len);
                                a2
                            }
                        }
                    }
                    call::TERM_PALETTE_SET => {
                        let len = if a2 <= 256 { a2 as usize * 3 } else { usize::MAX };
                        match readable(rom, ram, a1, len).filter(|_| a0 < 256 && a2 <= 256) {
                            None => ERR_BUFFER,
                            Some(bytes) => {
                                t.palette_set(a0 as usize, bytes);
                                units += copy_charge(len);
                                a2
                            }
                        }
                    }
                    call::TERM_SIZE => {
                        r1 = Some(ROWS as u32);
                        COLS as u32
                    }
                    _ => ERR_UNKNOWN,
                }
            }
            3 => {
                let v = self.vector.as_mut().unwrap();
                match n {
                    // A list not wholly in RAM is rejected like a bad length.
                    call::VEC_SUBMIT => match ram_range(ram.len(), a0, a1 as usize) {
                        Some(at) if v.submit(&ram[at..at + a1 as usize]) == crate::vector::RESULT_DONE => {
                            units += copy_charge(a1 as usize);
                            crate::vector::RESULT_DONE
                        }
                        _ => crate::vector::RESULT_REJECTED,
                    },
                    call::VEC_CLEAR => {
                        v.clear();
                        0
                    }
                    call::VEC_STYLE => v.set_style(a0),
                    call::VEC_PERSISTENCE => v.set_persistence(a0),
                    call::VEC_FILL => v.set_fill(a0),
                    _ => ERR_UNKNOWN,
                }
            }
            crate::vector3::SLOT => {
                let v = self.vector3.as_mut().unwrap();
                match n {
                    call::V3_SUBMIT => match ram_range(ram.len(), a0, a1 as usize) {
                        Some(at) if v.submit(&ram[at..at + a1 as usize]) == crate::vector::RESULT_DONE => {
                            units += copy_charge(a1 as usize);
                            crate::vector::RESULT_DONE
                        }
                        _ => crate::vector::RESULT_REJECTED,
                    },
                    call::V3_CLEAR => {
                        v.clear();
                        0
                    }
                    // The style and persistence are device 3's (SPEC 13.2).
                    call::V3_STYLE => self.vector.as_mut().unwrap().set_style(a0),
                    call::V3_PERSISTENCE => self.vector.as_mut().unwrap().set_persistence(a0),
                    _ => ERR_UNKNOWN,
                }
            }
            4 if n == call::CONTENT_SUBMIT => {
                let (handle, taken) = self.content.as_mut().unwrap().submit(a0, rom, ram);
                units += copy_charge(taken);
                handle
            }
            7 => {
                let page = self.page.as_mut().unwrap();
                match n {
                    call::PAGE_SUBMIT => {
                        // The length is checked first; then the buffer.
                        if a1 == 0 || a1 as usize > crate::page::MAX_DOCUMENT {
                            crate::page::RESULT_REJECTED
                        } else {
                            match readable(rom, ram, a0, a1 as usize) {
                                None => ERR_BUFFER,
                                Some(bytes) => {
                                    units += copy_charge(bytes.len());
                                    page.submit(bytes)
                                }
                            }
                        }
                    }
                    call::PAGE_CLEAR => {
                        page.clear();
                        0
                    }
                    call::PAGE_REGION => {
                        page.set_region(a0, a1, a2, a3);
                        0
                    }
                    call::PAGE_SCROLL => {
                        page.set_scroll(a0);
                        0
                    }
                    call::PAGE_BACKGROUND => {
                        page.set_background(a0);
                        0
                    }
                    call::PAGE_LOCATE => page.locate(a0),
                    _ => ERR_UNKNOWN,
                }
            }
            crate::sound::SLOT => {
                let s = self.sound.as_mut().unwrap();
                match n {
                    // SPEC 18.2: a bad length or a list not wholly in ROM
                    // or RAM is rejected with 1; the list's bytes cost 2
                    // each, as `write`'s.
                    call::SOUND_SUBMIT => match readable(rom, ram, a0, a1 as usize).filter(|_| (1..=crate::sound::MAX_LIST as u32).contains(&a1)) {
                        Some(list) => {
                            units += a1 * charge::WRITE_BYTE;
                            s.submit(list)
                        }
                        None => crate::sound::RESULT_REJECTED,
                    },
                    call::SOUND_WAVE => match readable(rom, ram, a1, crate::sound::WAVE_LEN).filter(|_| (a0 as usize) < crate::sound::WAVES) {
                        None => ERR_BUFFER,
                        Some(bytes) => {
                            s.load_wave(a0 as usize, bytes);
                            units += copy_charge(bytes.len());
                            0
                        }
                    },
                    call::SOUND_SAMPLE => {
                        let ok = a2 as usize <= crate::sound::MAX_SAMPLE && (a0 as usize) < crate::sound::SAMPLE_SLOTS;
                        match readable(rom, ram, a1, a2 as usize).filter(|_| ok) {
                            None => ERR_BUFFER,
                            Some(bytes) => {
                                s.load_sample(a0 as usize, bytes, a3);
                                units += copy_charge(bytes.len());
                                0
                            }
                        }
                    }
                    call::SOUND_INFO => {
                        r1 = Some(crate::sound::CAPS);
                        crate::sound::VOICES as u32
                    }
                    _ => ERR_UNKNOWN,
                }
            }
            _ => ERR_UNKNOWN,
        };
        Ok(CallOutcome { a0: r0, a1: r1, charge: units })
    }

    fn end_frame(&mut self, _frame: u32, _stopped: bool, _mem: GuestMem) -> Result<(), String> {
        // SPEC 18.5: the frame's samples, with the registers as the
        // frame's calls left them.
        if let Some(s) = &mut self.sound {
            s.render_frame();
        }
        Ok(())
    }

    fn take_audio(&mut self) -> Option<Vec<i16>> {
        self.sound.as_mut().and_then(|s| s.take_frame().map(<[i16]>::to_vec))
    }

    fn write_state(&mut self, slot: u8, out: &mut Vec<u8>) -> Result<(), String> {
        match slot {
            2 => self.terminal.as_ref().ok_or("no terminal")?.write_state(out),
            3 => self.vector.as_ref().ok_or("no vector display")?.write_state(out),
            4 => self.content.as_ref().ok_or("no content device")?.write_state(out),
            7 => self.page.as_ref().ok_or("no page")?.write_state(out),
            crate::vector3::SLOT => self.vector3.as_ref().ok_or("no 3D vector display")?.write_state(out),
            crate::sound::SLOT => self.sound.as_ref().ok_or("no sound device")?.write_state(out),
            _ => return Err(format!("no state section for slot {slot}")),
        }
        Ok(())
    }

    fn list3(&self) -> Cow<'_, [u8]> {
        Cow::Borrowed(self.vector3.as_ref().map_or(&[][..], Vector3::list))
    }

    fn restore_state(&mut self, slot: u8, bytes: &[u8], ram_len: usize) -> Result<usize, RestoreError> {
        let mut r = Reader::new(bytes);
        match slot {
            2 => self.terminal.as_mut().ok_or(bad("terminal present"))?.restore(&mut r)?,
            3 => {
                let v = self.vector.as_mut().ok_or(bad("vector display present"))?;
                v.restore(&mut r)?;
                // Without device 3 the fill mode and the 2D list cannot be set.
                if !self.lists_vector && (!v.fill() || !v.list().is_empty()) {
                    return Err(bad("a fill mode or 2D list without device 3"));
                }
            }
            4 => self.content.as_mut().ok_or(bad("content present"))?.restore(&mut r, ram_len)?,
            7 => self.page.as_mut().ok_or(bad("page present"))?.restore(&mut r)?,
            crate::vector3::SLOT => self.vector3.as_mut().ok_or(bad("3D display present"))?.restore(&mut r)?,
            crate::sound::SLOT => self.sound.as_mut().ok_or(bad("sound present"))?.restore(&mut r)?,
            _ => return Err(bad("no such device")),
        }
        Ok(r.position())
    }

    fn new_machine(&self, _serial: u32, image: &crate::Image, identity: Option<&ProgramId>) -> Result<crate::Machine, StartFailure> {
        let mut m = crate::Machine::new(image);
        m.set_identity(identity.cloned());
        Ok(m)
    }
}
