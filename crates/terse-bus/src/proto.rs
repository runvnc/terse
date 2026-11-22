//! The device bus messages (DEVICES.md 1, 5): their types, the call table,
//! and little helpers to write and read them. The same bytes travel on
//! both transports.

/// Protocol version (DEVICES.md 1.1).
pub const VERSION: u8 = 1;
/// The longest message (what follows its length).
pub const MAX_MESSAGE: usize = 16 << 20;
/// Bytes of a message's address: instance (u32), slot (u8), 3 reserved.
pub const ADDRESS: usize = 8;

pub mod msg {
    pub const HELLO: u8 = 0x01;
    pub const RESET: u8 = 0x02;
    pub const OPEN: u8 = 0x03;
    pub const CLOSE: u8 = 0x04;
    pub const CALL: u8 = 0x10;
    pub const BATCH: u8 = 0x11;
    pub const READ: u8 = 0x20;
    pub const BEGIN: u8 = 0x30;
    pub const END: u8 = 0x31;
    pub const STATE: u8 = 0x40;
    pub const RESTORE: u8 = 0x41;
    pub const GET: u8 = 0x42;
    pub const SET: u8 = 0x43;
    pub const SERVICE: u8 = 0x44;
    pub const PRESENT: u8 = 0x50;
    pub const SHUTDOWN: u8 = 0x60;
    pub const ERROR: u8 = 0x7F;
    /// A reply's type: the request's with bit 7 set.
    pub const REPLY: u8 = 0x80;
    pub const DATA: u8 = 0xA0;
    /// CALL's reply.
    pub const RESULT: u8 = CALL | REPLY;
}

/// HELLO flags.
pub mod hello_flags {
    pub const SHM: u32 = 1 << 0;
    pub const PRESENTS: u32 = 1 << 1;
    pub const REPLAY: u32 = 1 << 2;
}

/// Event positions in INPUTS (DEVICES.md 1.4), the flag of a pair, and an
/// event with its position: the machine's own (terse-core's `devices`).
pub use terse_core::devices::{EVENT_PAIR, PosEvent, position};

/// Transports (HELLO reply byte 2).
pub const TRANSPORT_SOCKET: u8 = 0;
pub const TRANSPORT_SHM: u8 = 1;

/// Where the RAM starts in a memory file (DEVICES.md 4.1).
pub const MEMORY_RAM_OFFSET: usize = 131_072;

/// Whom a message is for: a machine instance and one of the driver's
/// slots (0: the instance as a whole; HELLO, BATCH and SHUTDOWN are the
/// process's and have instance 0 and slot 0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Addr {
    pub instance: u32,
    pub slot: u8,
}

impl Addr {
    pub const PROCESS: Addr = Addr { instance: 0, slot: 0 };

    pub fn new(instance: u32, slot: u8) -> Self {
        Addr { instance, slot }
    }

    /// The instance as a whole.
    pub fn of(instance: u32) -> Self {
        Addr { instance, slot: 0 }
    }
}

/// A message being written: its length is filled in by `finish`.
pub struct Writer<'a> {
    out: &'a mut Vec<u8>,
    start: usize,
}

impl<'a> Writer<'a> {
    /// Starts a message of type `kind` for `to` at the end of `out`.
    pub fn new(out: &'a mut Vec<u8>, to: Addr, kind: u8) -> Self {
        let start = out.len();
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&to.instance.to_le_bytes());
        out.extend_from_slice(&[to.slot, 0, 0, 0, kind]);
        Writer { out, start }
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.out.push(v);
        self
    }

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.out.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.out.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.out.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.out.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.out.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.out.extend_from_slice(b);
        self
    }

    /// A u16 length and the bytes.
    pub fn str(&mut self, b: &[u8]) -> &mut Self {
        self.u16(b.len() as u16).bytes(b)
    }

    /// A u32 length and the bytes.
    pub fn blob(&mut self, b: &[u8]) -> &mut Self {
        self.u32(b.len() as u32).bytes(b)
    }

    pub fn finish(&mut self) {
        let n = (self.out.len() - self.start - 4) as u32;
        self.out[self.start..self.start + 4].copy_from_slice(&n.to_le_bytes());
    }
}

/// Reads a message body.
#[derive(Clone)]
pub struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

#[derive(Debug)]
pub struct Short;

impl std::fmt::Display for Short {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("message cut short")
    }
}

impl From<Short> for String {
    fn from(_: Short) -> String {
        "message cut short".into()
    }
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Reader { b, at: 0 }
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], Short> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.b.len()).ok_or(Short)?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8, Short> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, Short> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> Result<u32, Short> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> Result<i32, Short> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn i64(&mut self) -> Result<i64, Short> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn f32(&mut self) -> Result<f32, Short> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn str(&mut self) -> Result<&'a [u8], Short> {
        let n = self.u16()? as usize;
        self.take(n)
    }

    /// A u16-length string, as UTF-8 (lossy).
    pub fn string(&mut self) -> Result<String, Short> {
        Ok(String::from_utf8_lossy(self.str()?).into_owned())
    }

    pub fn blob(&mut self) -> Result<&'a [u8], Short> {
        let n = self.u32()? as usize;
        self.take(n)
    }

    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.b[self.at..];
        self.at = self.b.len();
        s
    }

    pub fn skip(&mut self, n: usize) -> Result<(), Short> {
        self.take(n).map(|_| ())
    }
}

/// A received message: whom it is for and its body (from the type byte).
#[derive(Clone, Copy, Debug)]
pub struct Msg<'a> {
    pub to: Addr,
    pub body: &'a [u8],
}

impl Msg<'_> {
    pub fn kind(&self) -> u8 {
        self.body.first().copied().unwrap_or(0)
    }
}

/// A message's address and body, from what follows its length.
pub fn split(m: &[u8]) -> Result<Msg<'_>, Short> {
    if m.len() < ADDRESS + 1 {
        return Err(Short);
    }
    let instance = u32::from_le_bytes(m[0..4].try_into().unwrap());
    Ok(Msg { to: Addr { instance, slot: m[4] }, body: &m[ADDRESS..] })
}

/// Splits a packet (framed messages back to back) into messages.
pub fn messages(packet: &[u8]) -> impl Iterator<Item = Result<Msg<'_>, Short>> {
    let mut at = 0;
    std::iter::from_fn(move || {
        if at >= packet.len() {
            return None;
        }
        let Some(head) = packet.get(at..at + 4) else {
            at = packet.len();
            return Some(Err(Short));
        };
        let n = u32::from_le_bytes(head.try_into().unwrap()) as usize;
        let Some(m) = packet.get(at + 4..at + 4 + n) else {
            at = packet.len();
            return Some(Err(Short));
        };
        at += 4 + n;
        Some(split(m))
    })
}

/// Buffer access bits (DEVICES.md 5).
pub mod access {
    pub const READ: u8 = 1;
    pub const WRITE: u8 = 2;
    pub const ROM: u8 = 4;
}

/// Entry flags.
pub mod entry_flags {
    pub const SETS_A1: u8 = 1;
    /// Deferred (SPEC §charges): computable by the core, so batchable; without
    /// it a call is answered.
    pub const BATCHABLE: u8 = 2;
    pub const BUFFER: u8 = 4;
}

/// Result rules for batchable calls.
pub mod rule {
    pub const NONE: u8 = 0;
    pub const CONSTANT: u8 = 1;
    pub const ARGUMENT: u8 = 2;
    pub const AT_MOST: u8 = 3;
}

pub const NO_ARG: u8 = 0xFF;
pub const ENTRY_BYTES: usize = 64;

/// One call table entry (DEVICES.md 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallEntry {
    pub function: u8,
    pub flags: u8,
    pub buf_addr_arg: u8,
    pub buf_len_arg: u8,
    pub buf_scale: u32,
    pub buf_min: u32,
    pub buf_max: u32,
    pub access: u8,
    pub check_arg: u8,
    pub check_max: u32,
    pub len_bad: u32,
    pub place_bad: u32,
    pub rule: u8,
    pub rule_arg: u8,
    pub value: u32,
    pub value2: u32,
    pub charge_const: u32,
    pub charge_per4: bool,
    pub charge_arg: u8,
    /// Units per byte of the buffer.
    pub charge_per_byte: u16,
    pub charge_lo: u32,
    pub charge_hi: u32,
    pub charge_extra: u32,
}

impl CallEntry {
    /// A call with no buffer, needing a reply.
    pub const fn plain(function: u8) -> Self {
        CallEntry {
            function,
            flags: 0,
            buf_addr_arg: 0,
            buf_len_arg: NO_ARG,
            buf_scale: 0,
            buf_min: 0,
            buf_max: 0,
            access: 0,
            check_arg: NO_ARG,
            check_max: 0,
            len_bad: terse_core::ERR_BUFFER,
            place_bad: terse_core::ERR_BUFFER,
            rule: rule::NONE,
            rule_arg: 0,
            value: 0,
            value2: 0,
            charge_const: 0,
            charge_per4: false,
            charge_arg: NO_ARG,
            charge_per_byte: 0,
            charge_lo: 0,
            charge_hi: 0,
            charge_extra: 0,
        }
    }

    /// Sets `a1` too.
    pub const fn sets_a1(mut self) -> Self {
        self.flags |= entry_flags::SETS_A1;
        self
    }

    /// A buffer at argument `addr` whose length is argument `len` × `scale`, `min` to `max` bytes.
    pub const fn buffer(mut self, addr: u8, len: u8, scale: u32, min: u32, max: u32, access: u8) -> Self {
        self.flags |= entry_flags::BUFFER;
        self.buf_addr_arg = addr;
        self.buf_len_arg = len;
        self.buf_scale = scale;
        self.buf_min = min;
        self.buf_max = max;
        self.access = access;
        self
    }

    /// A buffer of fixed length at argument `addr`.
    pub const fn fixed(mut self, addr: u8, len: u32, access: u8) -> Self {
        self = self.buffer(addr, NO_ARG, len, len, len, access);
        self
    }

    pub const fn results(mut self, len_bad: u32, place_bad: u32) -> Self {
        self.len_bad = len_bad;
        self.place_bad = place_bad;
        self
    }

    pub const fn check(mut self, arg: u8, max: u32) -> Self {
        self.check_arg = arg;
        self.check_max = max;
        self
    }

    /// Batchable, with result rule `rule` (argument `arg`, values) and a
    /// constant charge.
    pub const fn batch(mut self, rule: u8, arg: u8, value: u32, value2: u32, charge: u32) -> Self {
        self.flags |= entry_flags::BATCHABLE;
        self.rule = rule;
        self.rule_arg = arg;
        self.value = value;
        self.value2 = value2;
        self.charge_const = charge;
        self
    }

    pub const fn per4(mut self) -> Self {
        self.charge_per4 = true;
        self
    }

    /// Adds `units` per byte of the buffer to the charge.
    pub const fn per_byte(mut self, units: u16) -> Self {
        self.charge_per_byte = units;
        self
    }

    /// Adds `extra` to the charge when `lo` ≤ argument `arg` ≤ `hi`.
    pub const fn charge_when(mut self, arg: u8, lo: u32, hi: u32, extra: u32) -> Self {
        self.charge_arg = arg;
        self.charge_lo = lo;
        self.charge_hi = hi;
        self.charge_extra = extra;
        self
    }

    pub fn batchable(&self) -> bool {
        self.flags & entry_flags::BATCHABLE != 0
    }

    pub fn has_buffer(&self) -> bool {
        self.flags & entry_flags::BUFFER != 0
    }

    /// The units the core charges beyond the call's 4 before the driver's
    /// own charge (SPEC §charges): the rest of an answered call's 64.
    pub fn base_extra(&self) -> u32 {
        if self.batchable() { 0 } else { terse_core::charge::ANSWERED - terse_core::charge::CALL }
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        let start = out.len();
        out.extend_from_slice(&[self.function, self.flags, self.buf_addr_arg, self.buf_len_arg]);
        for v in [self.buf_scale, self.buf_min, self.buf_max] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&[self.access, self.check_arg, 0, 0]);
        for v in [self.check_max, self.len_bad, self.place_bad] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&[self.rule, self.rule_arg, 0, 0]);
        for v in [self.value, self.value2, self.charge_const] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&[self.charge_per4 as u8, self.charge_arg]);
        out.extend_from_slice(&self.charge_per_byte.to_le_bytes());
        for v in [self.charge_lo, self.charge_hi, self.charge_extra] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        debug_assert_eq!(out.len() - start, ENTRY_BYTES);
    }

    pub fn decode(b: &[u8]) -> Result<Self, String> {
        if b.len() != ENTRY_BYTES {
            return Err("call table entry: wrong length".into());
        }
        let u = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        let e = CallEntry {
            function: b[0],
            flags: b[1],
            buf_addr_arg: b[2],
            buf_len_arg: b[3],
            buf_scale: u(4),
            buf_min: u(8),
            buf_max: u(12),
            access: b[16],
            check_arg: b[17],
            check_max: u(20),
            len_bad: u(24),
            place_bad: u(28),
            rule: b[32],
            rule_arg: b[33],
            value: u(36),
            value2: u(40),
            charge_const: u(44),
            charge_per4: b[48] & 1 != 0,
            charge_arg: b[49],
            charge_per_byte: u16::from_le_bytes([b[50], b[51]]),
            charge_lo: u(52),
            charge_hi: u(56),
            charge_extra: u(60),
        };
        let arg_ok = |a: u8| a < 6 || a == NO_ARG;
        if e.has_buffer() && (e.buf_addr_arg >= 6 || !arg_ok(e.buf_len_arg) || e.buf_min > e.buf_max) {
            return Err(format!("call table entry {}: bad buffer", e.function));
        }
        if e.has_buffer() && e.access & access::WRITE != 0 && e.access & access::ROM != 0 {
            return Err(format!("call table entry {}: a written buffer cannot lie in ROM", e.function));
        }
        if !arg_ok(e.check_arg) || !arg_ok(e.charge_arg) || e.rule_arg >= 6 || e.rule > rule::AT_MOST {
            return Err(format!("call table entry {}: bad argument number", e.function));
        }
        if e.batchable() && (e.rule == rule::NONE || (e.has_buffer() && e.access & access::WRITE != 0)) {
            return Err(format!("call table entry {}: batchable without a result rule, or with a written buffer", e.function));
        }
        Ok(e)
    }

    /// The buffer's length for `args`: `None` if it cannot be computed in
    /// range (DEVICES.md 5 check 1).
    pub fn buffer_len(&self, args: &[u32; 6]) -> Option<u32> {
        let len = if self.buf_len_arg == NO_ARG { self.buf_scale as u64 } else { args[self.buf_len_arg as usize] as u64 * self.buf_scale as u64 };
        (self.buf_min as u64..=self.buf_max as u64).contains(&len).then_some(len as u32)
    }

    /// The result and charge of a batchable call (DEVICES.md 6), given its
    /// buffer's length.
    pub fn batch_outcome(&self, args: &[u32; 6], buf_len: u32) -> (u32, Option<u32>, u32) {
        let a = args[self.rule_arg as usize];
        let (r0, r1) = match self.rule {
            rule::CONSTANT => (self.value, (self.flags & entry_flags::SETS_A1 != 0).then_some(self.value2)),
            rule::ARGUMENT => (a, None),
            _ => ((a > self.value) as u32, None),
        };
        let mut charge = self.charge_const;
        if self.charge_per4 {
            charge += buf_len.div_ceil(4);
        }
        charge += buf_len * self.charge_per_byte as u32;
        if self.charge_arg != NO_ARG && (self.charge_lo..=self.charge_hi).contains(&args[self.charge_arg as usize]) {
            charge += self.charge_extra;
        }
        (r0, r1, charge)
    }
}

/// A layer a driver declares: ID and z.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerDecl {
    pub id: u8,
    pub z: u8,
}

/// The standard z values (DEVICES.md 7).
pub mod z {
    pub const TERMINAL_BACKGROUNDS: u8 = 10;
    pub const PAGE: u8 = 20;
    pub const VECTOR: u8 = 30;
    pub const VECTOR3: u8 = 35;
    pub const TERMINAL_CHARACTERS: u8 = 40;
}

/// The properties of the reference drivers and the core (DEVICES.md 7.2):
/// named per-machine values one end publishes and others subscribe to,
/// carried by the core.
pub mod property {
    /// Published by the vector display: 1 if figures and filled shapes are
    /// drawn as outlines (fill mode 1, or the phosphor style), else 0; the
    /// page subscribes (SPEC §page.layout).
    pub const OUTLINES: &str = "vector.outlines";
    /// Published by the 3D display: its list (SPEC §vector3d), which the core
    /// needs for device 8 and the shell's scene (SPEC §machines.calls, §machines.screens and §machines.input).
    pub const LIST3: &str = "vector3.list";
    /// Published by the 2D display: its state as `STATE.md`'s vector
    /// section (style, persistence, fill mode, the list's length and the
    /// list), which the core needs for the system ink (SPEC §machines.ink)
    /// and to draw the phosphor style's beam (DEVICES.md 7.1).
    pub const LIST2: &str = "vector.list";
    /// Published by the core for the terminal, page and vector drivers: 1
    /// when the machine is shown in the phosphor style and its layers are
    /// light (DEVICES.md 7.1: no backgrounds, straight colour, the vector
    /// displays' layers left empty), else 0 or empty.
    pub const LIGHT: &str = "core.light";
    /// Published by a vector driver that draws the phosphor style itself
    /// (DEVICES.md 7.2): 1 when it does. In the phosphor style its layers
    /// are then light like the text's, and the core adds the light of
    /// every light layer as it is instead of drawing its own tube.
    pub const TUBE: &str = "vector.tube";
    /// Published by the core for the drivers that draw a look of their own
    /// (DEVICES.md 7.2): how much of it the host sheds because its frames
    /// run long (PERFORMANCE.md R15 step 1), a level (u8) from 0 (none) to
    /// 3 (the most), as terse-host's phosphor look sheds; empty: 0.
    pub const SHED: &str = "core.shed";
    /// Published by the core for the vector driver: the ink caps the
    /// machine's lists are drawn within, its share of the system ink (SPEC
    /// §machines.ink): the 2D cap (u64) and the 3D cap (f64); empty: the
    /// devices' own.
    pub const CAPS: &str = "core.caps";
    /// Published by the core: the eye (two f32, screen units from the
    /// reference eye, SPEC §vector3d.space).
    pub const EYE: &str = "core.eye";
    /// Published by the core for the 3D display of a machine whose scene
    /// has Screens in effect (SPEC §machines.screens): which (u8) and the view (12 f64:
    /// the linear part by rows, then the translation); empty otherwise.
    pub const VIEW: &str = "core.view";
    /// Published by the core: the line the terminal shows over its bottom
    /// row when the machine has stopped with a fault or lost a device
    /// (UTF-8; empty: none).
    pub const STATUS: &str = "core.status";
    /// Published by the content driver (instance 0): the consent question
    /// waiting (DEVICES.md 7.2), empty if none.
    pub const PROMPT: &str = "content.prompt";
    /// Published by the core for the content driver (instance 0): the
    /// user's answer to the question shown (1 allow, 0 refuse), with the
    /// question's number.
    pub const ANSWER: &str = "core.answer";
    /// Published by the sound driver: the frame counter after the frame
    /// it rendered last (u32), then that frame's 800 sample pairs (i16,
    /// left and right interleaved, SPEC §sound.presentation); empty before the first.
    /// The core reads it after each of the machine's frames.
    pub const SOUND_FRAME: &str = "sound.frame";
}

/// The host services of the reference content driver (DEVICES.md 9).
pub mod service {
    pub const FETCH_IMAGE: &str = "image.fetch";
    pub const FETCH_PROGRAM: &str = "program.fetch";
    pub const SNAPSHOT_SAVE: &str = "snapshot.save";
    pub const SNAPSHOT_LOAD: &str = "snapshot.load";
    pub const SNAPSHOT_DELETE: &str = "snapshot.delete";
    pub const SNAPSHOT_TABLE: &str = "snapshot.table";
}

/// One slot a driver gives (HELLO reply).
#[derive(Clone, Debug, Default)]
pub struct SlotInfo {
    pub slot: u8,
    pub version: u8,
    pub layers: Vec<LayerDecl>,
    pub table: Vec<CallEntry>,
}

/// The driver's HELLO reply, as a driver describes itself.
#[derive(Clone, Debug, Default)]
pub struct DeviceInfo {
    pub name: String,
    pub native: (u16, u16),
    pub slots: Vec<SlotInfo>,
    /// Properties it publishes and subscribes to (section 7.2).
    pub publishes: Vec<String>,
    pub subscribes: Vec<String>,
    /// Host services it gives (section 9).
    pub services: Vec<String>,
    /// Its identity (section 1.1, `PERFORMANCE.md` R13): the version and
    /// build identifier after the name; empty if not given (an older
    /// driver's reply ends after the services).
    pub version: String,
    pub build: String,
    /// An experimental driver's declaration (DEVICES.md 15.1), after the
    /// build: empty for every other driver, and then not sent.
    pub experimental: Vec<Experimental>,
}

/// What an experimental driver declares of its device (DEVICES.md 15.1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Experimental {
    pub slot: u8,
    pub name: String,
    /// The most bytes its state section can have.
    pub state_max: u32,
}

/// The limits of DEVICES.md 15.2.
pub mod limits {
    pub const CALLS: usize = 16;
    pub const BUFFER: u32 = 4096;
    pub const CONSTANT: u32 = 4096;
    pub const PER_BYTE: u16 = 2;
    pub const EXTRA: u32 = 4096;
    pub const STATE: u32 = terse_core::image::EXPERIMENTAL_STATE_MAX as u32;
}

impl DeviceInfo {
    /// Checks an experimental driver's reply against DEVICES.md 15.1 and
    /// 15.2: it serves exactly `slot`, as `name` (any valid name if
    /// `name` is empty), within the limits.
    pub fn check_experimental(&self, slot: u8, name: &str) -> Result<(), String> {
        let [x] = self.experimental.as_slice() else { return Err("it did not declare one experimental device (DEVICES.md 15.1)".into()) };
        if x.slot != slot || self.slots.len() != 1 || self.slots[0].slot != slot {
            return Err(format!("it answered for slots other than {slot}"));
        }
        if !terse_core::image::valid_experimental_name(&x.name) {
            return Err(format!("it calls its device {:?}, not a valid name", x.name));
        }
        if !name.is_empty() && x.name != name {
            return Err(format!("it serves {}, not {name}", x.name));
        }
        if x.state_max > limits::STATE {
            return Err(format!("its state may be {} bytes, more than {}", x.state_max, limits::STATE));
        }
        let s = &self.slots[0];
        if !s.layers.is_empty() || !self.publishes.is_empty() || !self.subscribes.is_empty() || !self.services.is_empty() {
            return Err("an experimental driver declares no layers, properties or services".into());
        }
        if s.table.is_empty() || s.table.len() > limits::CALLS {
            return Err(format!("its call table has {} calls, not 1 to {}", s.table.len(), limits::CALLS));
        }
        for (i, e) in s.table.iter().enumerate() {
            let f = e.function;
            if s.table[..i].iter().any(|o| o.function == f) {
                return Err(format!("function {f} is in its call table twice"));
            }
            if e.has_buffer() && e.buf_max > limits::BUFFER {
                return Err(format!("function {f}: a buffer of up to {} bytes, more than {}", e.buf_max, limits::BUFFER));
            }
            if e.has_buffer() && e.buf_max > 0 && !e.charge_per4 && e.charge_per_byte == 0 {
                return Err(format!("function {f}: a buffer must be charged at least 1 per 4 bytes"));
            }
            if e.charge_const > limits::CONSTANT || e.charge_per_byte > limits::PER_BYTE || (e.charge_arg != NO_ARG && e.charge_extra > limits::EXTRA) {
                return Err(format!("function {f}: a charge above the limits of DEVICES.md 15.2"));
            }
        }
        Ok(())
    }
}

/// The HELLO a driver receives.
#[derive(Clone, Debug, Default)]
pub struct Hello {
    pub flags: u32,
    /// The slots the core asks this driver for.
    pub slots: Vec<u8>,
    pub session_dir: String,
    /// The driver's bus file (section 4.2); empty if shared memory is not
    /// offered.
    pub bus_path: String,
    /// The experimental devices asked for (DEVICES.md 15.1): slot and
    /// name, an empty name asking the driver which it is. Empty for every
    /// other driver, and then not sent.
    pub experimental: Vec<(u8, String)>,
}

impl Hello {
    pub fn encode(&self, out: &mut Vec<u8>) {
        let mut w = Writer::new(out, Addr::PROCESS, msg::HELLO);
        w.u8(VERSION).u16(0).u32(self.flags).u8(self.slots.len() as u8).bytes(&self.slots).str(self.session_dir.as_bytes()).str(self.bus_path.as_bytes());
        if !self.experimental.is_empty() {
            w.u8(self.experimental.len() as u8);
            for (slot, name) in &self.experimental {
                w.u8(*slot).str(name.as_bytes());
            }
        }
        w.finish();
    }

    pub fn decode(body: &[u8]) -> Result<Self, String> {
        let mut r = Reader::new(body);
        if r.u8()? != msg::HELLO {
            return Err("expected HELLO".into());
        }
        let version = r.u8()?;
        if version != VERSION {
            return Err(format!("protocol version {version}, not {VERSION}"));
        }
        r.skip(2)?;
        let flags = r.u32()?;
        let n = r.u8()? as usize;
        let slots = r.take(n)?.to_vec();
        let session_dir = r.string()?;
        let bus_path = r.string()?;
        let mut experimental = Vec::new();
        if !r.clone().rest().is_empty() {
            for _ in 0..r.u8()? {
                experimental.push((r.u8()?, r.string()?));
            }
        }
        Ok(Hello { flags, slots, session_dir, bus_path, experimental })
    }
}

fn names(w: &mut Writer, v: &[String]) {
    w.u8(v.len() as u8);
    for s in v {
        w.str(s.as_bytes());
    }
}

fn read_names(r: &mut Reader) -> Result<Vec<String>, Short> {
    let n = r.u8()?;
    (0..n).map(|_| r.string()).collect()
}

impl DeviceInfo {
    pub fn encode(&self, transport: u8, out: &mut Vec<u8>) {
        let mut w = Writer::new(out, Addr::PROCESS, msg::HELLO | msg::REPLY);
        w.u8(VERSION).u8(transport).u8(0).str(self.name.as_bytes()).u16(self.native.0).u16(self.native.1);
        w.u8(self.slots.len() as u8);
        for s in &self.slots {
            w.u8(s.slot).u8(s.version).u8(s.layers.len() as u8);
            for l in &s.layers {
                w.u8(l.id).u8(l.z);
            }
            w.u8(s.table.len() as u8);
            let mut t = Vec::new();
            for e in &s.table {
                e.encode(&mut t);
            }
            w.bytes(&t);
        }
        names(&mut w, &self.publishes);
        names(&mut w, &self.subscribes);
        names(&mut w, &self.services);
        w.str(self.version.as_bytes()).str(self.build.as_bytes());
        if !self.experimental.is_empty() {
            w.u8(self.experimental.len() as u8);
            for x in &self.experimental {
                w.u8(x.slot).str(x.name.as_bytes()).u32(x.state_max);
            }
        }
        w.finish();
    }

    /// The reply's transport and the driver's description.
    pub fn decode(body: &[u8]) -> Result<(u8, DeviceInfo), String> {
        let mut r = Reader::new(body);
        match r.u8()? {
            t if t == msg::HELLO | msg::REPLY => {}
            msg::ERROR => return Err(String::from_utf8_lossy(r.rest()).into_owned()),
            t => return Err(format!("expected a HELLO reply, got message {t:02x}")),
        }
        let version = r.u8()?;
        if version != VERSION {
            return Err(format!("protocol version {version}, not {VERSION}"));
        }
        let transport = r.u8()?;
        r.skip(1)?;
        let name = r.string()?;
        let native = (r.u16()?, r.u16()?);
        let n = r.u8()?;
        let mut slots = Vec::new();
        for _ in 0..n {
            let (slot, version, k) = (r.u8()?, r.u8()?, r.u8()?);
            let mut layers = Vec::new();
            for _ in 0..k {
                layers.push(LayerDecl { id: r.u8()?, z: r.u8()? });
            }
            let m = r.u8()?;
            let mut table = Vec::new();
            for _ in 0..m {
                table.push(CallEntry::decode(r.take(ENTRY_BYTES)?)?);
            }
            slots.push(SlotInfo { slot, version, layers, table });
        }
        let publishes = read_names(&mut r)?;
        let subscribes = read_names(&mut r)?;
        let services = read_names(&mut r)?;
        // The identity, if given; anything after it is for later versions.
        let (mut version, mut build) = (String::new(), String::new());
        let mut experimental = Vec::new();
        if !r.clone().rest().is_empty() {
            version = terse_perflog::identity::clean_field(&r.string()?);
            build = terse_perflog::identity::clean_field(&r.string()?);
            // An experimental driver's declaration (DEVICES.md 15.1). What
            // does not parse as one is a later field, ignored as before.
            let mut x = r.clone();
            let mut read = || -> Result<Vec<Experimental>, Short> {
                let n = x.u8()?;
                (0..n).map(|_| Ok(Experimental { slot: x.u8()?, name: x.string()?, state_max: x.u32()? })).collect()
            };
            if let Ok(v) = read() {
                experimental = v;
            }
        }
        Ok((transport, DeviceInfo { name, native, slots, publishes, subscribes, services, version, build, experimental }))
    }
}

/// OPEN (DEVICES.md 1.2): a machine instance a driver now serves.
#[derive(Clone, Debug, Default)]
pub struct Open {
    pub image_id: [u8; 32],
    pub rom_len: u32,
    pub ram_len: u32,
    /// The instance's memory file (section 4.1); empty without shared
    /// memory.
    pub memory_path: String,
    /// The driver's slots the image lists, with the versions it lists.
    pub listed: Vec<(u8, u8)>,
    /// The program's identity (SPEC §machines.calls): publisher and name.
    pub identity: Option<([u8; 32], Vec<u8>)>,
    pub rom: Vec<u8>,
}

impl Open {
    pub fn lists(&self, slot: u8) -> bool {
        self.listed.iter().any(|l| l.0 == slot)
    }

    pub fn encode(&self, instance: u32, out: &mut Vec<u8>) {
        let mut w = Writer::new(out, Addr::of(instance), msg::OPEN);
        w.u8(0).u16(0).bytes(&self.image_id).u32(self.rom_len).u32(self.ram_len).str(self.memory_path.as_bytes());
        w.u8(self.listed.len() as u8);
        for &(s, v) in &self.listed {
            w.u8(s).u8(v);
        }
        match &self.identity {
            Some((publisher, name)) => w.u8(1).bytes(publisher).str(name),
            None => w.u8(0),
        };
        w.bytes(&self.rom);
        w.finish();
    }

    pub fn decode(body: &[u8]) -> Result<Self, String> {
        let mut r = Reader::new(body);
        r.skip(4)?;
        let image_id = r.take(32)?.try_into().unwrap();
        let (rom_len, ram_len) = (r.u32()?, r.u32()?);
        let memory_path = r.string()?;
        let n = r.u8()?;
        let mut listed = Vec::new();
        for _ in 0..n {
            listed.push((r.u8()?, r.u8()?));
        }
        let identity = match r.u8()? {
            0 => None,
            _ => Some((r.take(32)?.try_into().unwrap(), r.str()?.to_vec())),
        };
        let rom = r.take(rom_len as usize)?.to_vec();
        Ok(Open { image_id, rom_len, ram_len, memory_path, listed, identity, rom })
    }
}

/// A CALL message (DEVICES.md 1.4).
pub fn encode_call(out: &mut Vec<u8>, to: Addr, function: u8, args: &[u32; 6], budget: i64, buf: &[u8]) {
    let mut w = Writer::new(out, to, msg::CALL);
    w.u8(function).u16(0);
    for a in args {
        w.u32(*a);
    }
    w.i64(budget).u32(buf.len() as u32).bytes(buf);
    w.finish();
}

/// A decoded CALL.
pub struct Call<'a> {
    pub function: u8,
    pub args: [u32; 6],
    pub budget: i64,
    pub inline: &'a [u8],
}

pub fn decode_call(body: &[u8]) -> Result<Call<'_>, Short> {
    let mut r = Reader::new(body);
    r.u8()?;
    let function = r.u8()?;
    r.skip(2)?;
    let mut args = [0; 6];
    for a in &mut args {
        *a = r.u32()?;
    }
    let budget = r.i64()?;
    let inline = r.blob()?;
    Ok(Call { function, args, budget, inline })
}

/// Little-endian u32 at `at`.
pub fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_entries_round_trip_and_rules() {
        let e = CallEntry::plain(7).buffer(1, 2, 4, 0, 9600, access::READ | access::ROM).check(0, 2399).batch(rule::ARGUMENT, 2, 0, 0, 0).per4();
        let mut b = Vec::new();
        e.encode(&mut b);
        assert_eq!(b.len(), ENTRY_BYTES);
        assert_eq!(CallEntry::decode(&b).unwrap(), e);
        let args = [5, 0x10_0000, 3, 0, 0, 0];
        assert_eq!(e.buffer_len(&args), Some(12));
        assert_eq!(e.buffer_len(&[0, 0, 2401, 0, 0, 0]), None);
        assert_eq!(e.batch_outcome(&args, 12), (3, None, 3));
        let cmd = CallEntry::plain(1).batch(rule::CONSTANT, 0, 0, 0, 0).charge_when(0, 1, 3, 150);
        assert_eq!(cmd.batch_outcome(&[2, 0, 0, 0, 0, 0], 0), (0, None, 150));
        assert_eq!(cmd.batch_outcome(&[4, 0, 0, 0, 0, 0], 0), (0, None, 0));
        let style = CallEntry::plain(2).batch(rule::AT_MOST, 0, 1, 0, 0);
        assert_eq!(style.batch_outcome(&[2, 0, 0, 0, 0, 0], 0).0, 1);
        let size = CallEntry::plain(9).sets_a1().batch(rule::CONSTANT, 0, 80, 30, 0);
        assert_eq!(size.batch_outcome(&[0; 6], 0), (80, Some(30), 0));
        // write: the length back, 2 per byte.
        let write = CallEntry::plain(0).buffer(0, 1, 1, 0, 4096, access::READ | access::ROM).batch(rule::ARGUMENT, 1, 0, 0, 0).per_byte(2);
        let mut b = Vec::new();
        write.encode(&mut b);
        assert_eq!(CallEntry::decode(&b).unwrap(), write);
        assert_eq!(write.batch_outcome(&[0x10_0000, 100, 0, 0, 0, 0], 100), (100, None, 200));
        assert_eq!((write.base_extra(), CallEntry::plain(3).base_extra()), (0, 60));
        // A written buffer in ROM, or batchable with a written buffer, is refused.
        let mut bad = Vec::new();
        CallEntry::plain(6).buffer(1, 2, 4, 0, 9600, access::WRITE | access::ROM).encode(&mut bad);
        assert!(CallEntry::decode(&bad).is_err());
    }

    /// DEVICES.md 15: an experimental driver's HELLO fields, and the limits.
    #[test]
    fn experimental_hello_and_limits() {
        let h = Hello { flags: 0, slots: vec![240], session_dir: "/s".into(), bus_path: String::new(), experimental: vec![(240, "example.dice".into())] };
        let mut out = Vec::new();
        h.encode(&mut out);
        let back = Hello::decode(split(&out[4..]).unwrap().body).unwrap();
        assert_eq!(back.experimental, h.experimental);
        // A HELLO with no experimental slot is byte for byte as before.
        let plain = Hello { experimental: vec![], ..h.clone() };
        let mut p = Vec::new();
        plain.encode(&mut p);
        assert_eq!(p.len() + 1 + 1 + 2 + 12, out.len());
        assert!(Hello::decode(split(&p[4..]).unwrap().body).unwrap().experimental.is_empty());
        let roll = CallEntry::plain(1).check(0, 255);
        let hist = CallEntry::plain(2).buffer(0, 1, 1, 0, 64, access::WRITE).per4();
        let info = DeviceInfo {
            name: "dice".into(),
            slots: vec![SlotInfo { slot: 240, version: 0, layers: vec![], table: vec![roll, hist] }],
            experimental: vec![Experimental { slot: 240, name: "example.dice".into(), state_max: 72 }],
            ..Default::default()
        };
        let mut out = Vec::new();
        info.encode(TRANSPORT_SOCKET, &mut out);
        let (_, d) = DeviceInfo::decode(split(&out[4..]).unwrap().body).unwrap();
        assert_eq!(d.experimental, info.experimental);
        assert_eq!(d.check_experimental(240, "example.dice"), Ok(()));
        assert_eq!(d.check_experimental(240, ""), Ok(()));
        assert!(d.check_experimental(240, "other.dice").is_err());
        assert!(d.check_experimental(241, "example.dice").is_err());
        let with = |f: &dyn Fn(&mut DeviceInfo)| {
            let mut d = info.clone();
            f(&mut d);
            d.check_experimental(240, "example.dice")
        };
        assert!(with(&|d| d.experimental[0].state_max = 16_385).is_err());
        assert!(with(&|d| d.slots[0].table[1].buf_max = 4097).is_err());
        assert!(with(&|d| d.slots[0].table[1].charge_per4 = false).is_err());
        assert_eq!(
            with(&|d| {
                d.slots[0].table[1].charge_per4 = false;
                d.slots[0].table[1].charge_per_byte = 1
            }),
            Ok(())
        );
        assert!(with(&|d| d.slots[0].table[0].charge_const = 4097).is_err());
        assert!(with(&|d| d.slots[0].table[1].charge_per_byte = 3).is_err());
        assert!(with(&|d| d.slots[0].table = vec![]).is_err());
        assert!(with(&|d| d.slots[0].table = vec![roll; 17]).is_err());
        assert!(with(&|d| d.slots[0].table.push(roll)).is_err());
        assert!(with(&|d| d.slots[0].layers.push(LayerDecl { id: 0, z: 30 })).is_err());
        assert!(with(&|d| d.publishes.push("x".into())).is_err());
        assert!(with(&|d| d.experimental.clear()).is_err());
    }

    #[test]
    fn hello_open_and_info_round_trip() {
        let h = Hello { flags: hello_flags::SHM, slots: vec![3, 9], session_dir: "/s".into(), bus_path: "/b".into(), experimental: vec![] };
        let mut out = Vec::new();
        h.encode(&mut out);
        let m: Vec<Msg> = messages(&out).map(Result::unwrap).collect();
        assert_eq!(m[0].to, Addr::PROCESS);
        let back = Hello::decode(m[0].body).unwrap();
        assert_eq!((back.flags, back.slots, back.session_dir), (hello_flags::SHM, vec![3, 9], "/s".to_string()));
        let open = Open {
            image_id: [7; 32],
            rom_len: 3,
            ram_len: 8192,
            memory_path: "/m".into(),
            listed: vec![(9, 0)],
            identity: Some(([1; 32], b"holo".to_vec())),
            rom: vec![1, 2, 3],
        };
        let mut out = Vec::new();
        open.encode(5, &mut out);
        let m = split(&out[4..]).unwrap();
        assert_eq!(m.to, Addr::of(5));
        let back = Open::decode(m.body).unwrap();
        assert_eq!((back.rom, back.listed, back.identity, back.memory_path), (vec![1, 2, 3], vec![(9, 0)], open.identity, "/m".to_string()));
        let info = DeviceInfo {
            name: "t".into(),
            native: (640, 480),
            slots: vec![
                SlotInfo { slot: 3, version: 0, layers: vec![LayerDecl { id: 0, z: 30 }], table: vec![CallEntry::plain(3).sets_a1()] },
                SlotInfo { slot: 9, version: 0, layers: vec![LayerDecl { id: 0, z: 35 }], table: vec![] },
            ],
            publishes: vec![property::OUTLINES.into(), property::LIST3.into()],
            subscribes: vec![property::EYE.into()],
            services: vec![],
            version: "0.1.0".into(),
            build: "b3:0011223344556677".into(),
            experimental: vec![],
        };
        let mut out = Vec::new();
        info.encode(TRANSPORT_SHM, &mut out);
        let (transport, d) = DeviceInfo::decode(split(&out[4..]).unwrap().body).unwrap();
        assert_eq!((transport, d.native, d.slots.len(), d.slots[1].layers[0].z), (TRANSPORT_SHM, (640, 480), 2, 35));
        assert_eq!((d.publishes, d.subscribes), (info.publishes.clone(), info.subscribes.clone()));
        assert_eq!((d.version.as_str(), d.build.as_str()), ("0.1.0", "b3:0011223344556677"));
        // An older driver's reply ends after the services: no identity.
        let n = out.len() - (2 + 5 + 2 + 19);
        let mut old = out[..n].to_vec();
        old[..4].copy_from_slice(&((n - 4) as u32).to_le_bytes());
        let (_, d) = DeviceInfo::decode(split(&old[4..]).unwrap().body).unwrap();
        assert_eq!((d.version.as_str(), d.build.as_str(), d.services.len()), ("", "", 0));
        // A later one may say more after the build; a field with a space
        // is not kept.
        let mut later = DeviceInfo { build: "a b".into(), ..info.clone() };
        later.version = "0.2".into();
        let mut out = Vec::new();
        later.encode(TRANSPORT_SOCKET, &mut out);
        out.extend_from_slice(&[9, 9, 9]);
        let n = out.len() - 4;
        out[..4].copy_from_slice(&(n as u32).to_le_bytes());
        let (_, d) = DeviceInfo::decode(split(&out[4..]).unwrap().body).unwrap();
        assert_eq!((d.version.as_str(), d.build.as_str()), ("0.2", ""));
    }
}
