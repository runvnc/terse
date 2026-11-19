//! The device bus messages (DEVICES.md 1, 5): their types, the call table,
//! and little helpers to write and read them. The same bytes travel on
//! both transports.

/// Protocol version (DEVICES.md 1.1).
pub const VERSION: u8 = 0;
/// The longest message body.
pub const MAX_MESSAGE: usize = 16 << 20;

pub mod msg {
    pub const HELLO: u8 = 0x01;
    pub const RESET: u8 = 0x02;
    pub const CALL: u8 = 0x10;
    pub const BATCH: u8 = 0x11;
    pub const READ: u8 = 0x20;
    pub const BEGIN: u8 = 0x30;
    pub const END: u8 = 0x31;
    pub const STATE: u8 = 0x40;
    pub const RESTORE: u8 = 0x41;
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

/// Transports (HELLO reply byte 4).
pub const TRANSPORT_SOCKET: u8 = 0;
pub const TRANSPORT_SHM: u8 = 1;

/// Where the RAM starts in the memory file (DEVICES.md 4.1).
pub const MEMORY_RAM_OFFSET: usize = 131_072;

/// A message being written: the 4-byte length is filled in by `finish`.
pub struct Writer<'a> {
    out: &'a mut Vec<u8>,
    start: usize,
}

impl<'a> Writer<'a> {
    /// Starts a message of type `kind` at the end of `out`.
    pub fn new(out: &'a mut Vec<u8>, kind: u8) -> Self {
        let start = out.len();
        out.extend_from_slice(&[0, 0, 0, 0, kind]);
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

    /// Where the next byte goes (to patch a count later).
    pub fn mark(&self) -> usize {
        self.out.len()
    }

    pub fn patch_u32(&mut self, at: usize, v: u32) {
        self.out[at..at + 4].copy_from_slice(&v.to_le_bytes());
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

/// Splits a packet (framed messages back to back) into message bodies.
pub fn messages(packet: &[u8]) -> impl Iterator<Item = Result<&[u8], Short>> {
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
        let Some(body) = packet.get(at + 4..at + 4 + n) else {
            at = packet.len();
            return Some(Err(Short));
        };
        at += 4 + n;
        Some(Ok(body))
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
    /// Deferred (SPEC 14): computable by the core, so batchable; without
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
    /// own charge (SPEC 14): the rest of an answered call's 64.
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

/// Presentation property keys of the reference drivers.
pub mod property {
    /// Slot 3: figures and filled shapes are drawn as outlines.
    pub const OUTLINES: u8 = 1;
}

/// The device's HELLO reply, as a driver describes itself.
#[derive(Clone, Debug, Default)]
pub struct DeviceInfo {
    pub version: u8,
    pub name: String,
    pub native: (u16, u16),
    pub layers: Vec<LayerDecl>,
    pub table: Vec<CallEntry>,
}

/// The HELLO a driver receives.
#[derive(Clone, Debug)]
pub struct Hello {
    pub slot: u8,
    pub version: u8,
    pub flags: u32,
    pub image_id: [u8; 32],
    pub rom_len: u32,
    pub ram_len: u32,
    pub memory_path: String,
    pub bus_path: String,
    pub session_dir: String,
    pub rom: Vec<u8>,
}

impl Hello {
    pub fn encode(&self, out: &mut Vec<u8>) {
        let mut w = Writer::new(out, msg::HELLO);
        w.u8(VERSION).u8(self.slot).u8(self.version).u32(self.flags).bytes(&self.image_id).u32(self.rom_len).u32(self.ram_len);
        w.str(self.memory_path.as_bytes()).str(self.bus_path.as_bytes()).str(self.session_dir.as_bytes()).bytes(&self.rom);
        w.finish();
    }

    pub fn decode(body: &[u8]) -> Result<Self, String> {
        let mut r = Reader::new(body);
        if r.u8()? != msg::HELLO {
            return Err("expected HELLO".into());
        }
        if r.u8()? != VERSION {
            return Err("protocol version".into());
        }
        let (slot, version, flags) = (r.u8()?, r.u8()?, r.u32()?);
        let image_id = r.take(32)?.try_into().unwrap();
        let (rom_len, ram_len) = (r.u32()?, r.u32()?);
        let s = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        let memory_path = s(r.str()?);
        let bus_path = s(r.str()?);
        let session_dir = s(r.str()?);
        let rom = r.take(rom_len as usize)?.to_vec();
        Ok(Hello { slot, version, flags, image_id, rom_len, ram_len, memory_path, bus_path, session_dir, rom })
    }
}

impl DeviceInfo {
    pub fn encode(&self, slot: u8, transport: u8, out: &mut Vec<u8>) {
        let mut w = Writer::new(out, msg::HELLO | msg::REPLY);
        w.u8(VERSION).u8(slot).u8(self.version).u8(transport).str(self.name.as_bytes());
        w.u16(self.native.0).u16(self.native.1).u8(self.layers.len() as u8);
        for l in &self.layers {
            w.u8(l.id).u8(l.z);
        }
        w.u8(self.table.len() as u8);
        let mut t = Vec::new();
        for e in &self.table {
            e.encode(&mut t);
        }
        w.bytes(&t);
        w.finish();
    }

    /// The reply's slot, transport and the device's description.
    pub fn decode(body: &[u8]) -> Result<(u8, u8, DeviceInfo), String> {
        let mut r = Reader::new(body);
        match r.u8()? {
            t if t == msg::HELLO | msg::REPLY => {}
            msg::ERROR => return Err(String::from_utf8_lossy(r.rest()).into_owned()),
            t => return Err(format!("expected a HELLO reply, got message {t:02x}")),
        }
        if r.u8()? != VERSION {
            return Err("protocol version".into());
        }
        let (slot, version, transport) = (r.u8()?, r.u8()?, r.u8()?);
        let name = String::from_utf8_lossy(r.str()?).into_owned();
        let native = (r.u16()?, r.u16()?);
        let k = r.u8()?;
        let mut layers = Vec::new();
        for _ in 0..k {
            layers.push(LayerDecl { id: r.u8()?, z: r.u8()? });
        }
        let m = r.u8()?;
        let mut table = Vec::new();
        for _ in 0..m {
            table.push(CallEntry::decode(r.take(ENTRY_BYTES)?)?);
        }
        Ok((slot, transport, DeviceInfo { version, name, native, layers, table }))
    }
}

/// A CALL message (DEVICES.md 1.3).
pub fn encode_call(out: &mut Vec<u8>, function: u8, args: &[u32; 6], budget: i64, buf: &[u8]) {
    let mut w = Writer::new(out, msg::CALL);
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

    #[test]
    fn hello_and_info_round_trip() {
        let h = Hello {
            slot: 2,
            version: 0,
            flags: hello_flags::SHM,
            image_id: [7; 32],
            rom_len: 3,
            ram_len: 8192,
            memory_path: "/m".into(),
            bus_path: "/b".into(),
            session_dir: "/s".into(),
            rom: vec![1, 2, 3],
        };
        let mut out = Vec::new();
        h.encode(&mut out);
        let body: Vec<&[u8]> = messages(&out).map(Result::unwrap).collect();
        let back = Hello::decode(body[0]).unwrap();
        assert_eq!((back.slot, back.rom, back.memory_path), (2, vec![1, 2, 3], "/m".to_string()));
        let info = DeviceInfo { version: 0, name: "t".into(), native: (640, 480), layers: vec![LayerDecl { id: 0, z: 10 }], table: vec![CallEntry::plain(3).sets_a1()] };
        let mut out = Vec::new();
        info.encode(2, TRANSPORT_SHM, &mut out);
        let (slot, transport, d) = DeviceInfo::decode(&out[4..]).unwrap();
        assert_eq!((slot, transport, d.native, d.layers, d.table), (2, TRANSPORT_SHM, (640, 480), info.layers, info.table));
    }
}
