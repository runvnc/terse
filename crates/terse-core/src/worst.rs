//! Worst legal frames: images that make a host draw as much as one frame
//! may ask of it, device by device (PERFORMANCE.md R10, `terse perf` W
//! rows), built without a compiler.
//!
//! An image here is a small hand-assembled RV32I program and a table of
//! calls. At reset it copies its ROM to RAM (a display list must lie in
//! RAM, SPEC §vector.calls); then each frame it makes the calls of entry
//! `frame mod K` of its table and waits for the next frame. A call may
//! first store `(frame × 16) & F0h` as a byte at a given address (the
//! x of a list's Origin or Translate), so a list changes every frame and
//! a host cannot keep the picture it drew before.

use crate::image::{DeviceReq, Image};

const ROM: u32 = 0x0001_0000;
const RAM: u32 = 0x0010_0000;

/// A call of a frame: `a7`, `a0`–`a3`, and the address of the byte
/// to store `(frame × 16) & F0h` at first (0: none).
#[derive(Clone, Copy, Debug)]
pub struct Call {
    pub a7: u32,
    pub args: [u32; 4],
    pub poke: u32,
}

impl Call {
    pub fn new(a7: u32, args: [u32; 4]) -> Self {
        Call { a7, args, poke: 0 }
    }
}

/// An image under construction: data blobs (copied to RAM at reset) and
/// the calls of each frame of the cycle.
#[derive(Default)]
pub struct Builder {
    data: Vec<u8>,
    frames: Vec<Vec<Call>>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `bytes` to the data; returns their address in RAM (where the
    /// program has copied them by its first frame's calls).
    pub fn blob(&mut self, bytes: &[u8]) -> u32 {
        while self.data.len() % 4 != 0 {
            self.data.push(0);
        }
        let at = self.data.len() as u32;
        self.data.extend_from_slice(bytes);
        // Placed after the code and the table, so its address is known
        // only at `build`; blobs are kept relative until then.
        at | 0x8000_0000
    }

    /// Adds a frame to the cycle.
    pub fn frame(&mut self, calls: Vec<Call>) {
        self.frames.push(calls);
    }

    /// The image file: `title`, the devices it requires, the program and
    /// its data.
    pub fn build(&self, title: &str, devices: &[DeviceReq]) -> Vec<u8> {
        let code = code();
        // ROM: code, table (K, K pointers, the entries), blobs.
        let table_at = code.len() as u32;
        let k = self.frames.len().max(1);
        let entries_len: usize = self.frames.iter().map(|f| (f.len() + 1) * 24).sum::<usize>();
        let blobs_at = (table_at as usize + 4 + 4 * k + entries_len).next_multiple_of(4) as u32;
        let addr = |v: u32| if v & 0x8000_0000 != 0 { RAM + blobs_at + (v & 0x7FFF_FFFF) } else { v };
        let mut rom = code;
        let mut table = Vec::new();
        table.extend_from_slice(&(k as u32).to_le_bytes());
        let mut at = table_at + 4 + 4 * k as u32;
        for f in self.frames.iter().chain((self.frames.is_empty()).then_some(&Vec::new())) {
            table.extend_from_slice(&(RAM + at).to_le_bytes());
            at += (f.len() as u32 + 1) * 24;
        }
        for f in self.frames.iter().chain((self.frames.is_empty()).then_some(&Vec::new())) {
            for c in f {
                let words = [c.a7, addr(c.args[0]), addr(c.args[1]), addr(c.args[2]), addr(c.poke), addr(c.args[3])];
                for w in words {
                    table.extend_from_slice(&w.to_le_bytes());
                }
            }
            table.extend_from_slice(&[0; 24]);
        }
        rom.extend_from_slice(&table);
        while rom.len() < blobs_at as usize {
            rom.push(0);
        }
        rom.extend_from_slice(&self.data);
        while rom.len() % 4 != 0 {
            rom.push(0);
        }
        // The code needs the ROM's end and the table's address in RAM.
        let len = rom.len() as u32;
        patch(&mut rom, len, RAM + table_at);
        let pages = (rom.len().div_ceil(8192) + 1).min(128) as u8;
        assert!(rom.len() <= 131_072, "ROM of {} bytes", rom.len());
        Image::build(title, pages, devices, &rom)
    }
}

/// An address in a blob, `offset` bytes into the blob at `blob`.
pub fn at(blob: u32, offset: u32) -> u32 {
    blob + offset
}

// ------------------------------------------------------------ the program

const T0: u32 = 5;
const T1: u32 = 6;
const T2: u32 = 7;
const T3: u32 = 28;
const S1: u32 = 9;
const S2: u32 = 18;
const A0: u32 = 10;
const A1: u32 = 11;
const A2: u32 = 12;
const A3: u32 = 13;
const A7: u32 = 17;

fn i_type(imm: i32, rs1: u32, f3: u32, rd: u32, op: u32) -> u32 {
    ((imm as u32 & 0xFFF) << 20) | rs1 << 15 | f3 << 12 | rd << 7 | op
}
fn s_type(imm: i32, rs2: u32, rs1: u32, f3: u32) -> u32 {
    let imm = imm as u32;
    ((imm >> 5) & 0x7F) << 25 | rs2 << 20 | rs1 << 15 | f3 << 12 | (imm & 0x1F) << 7 | 0x23
}
fn b_type(off: i32, rs2: u32, rs1: u32, f3: u32) -> u32 {
    let o = off as u32;
    ((o >> 12) & 1) << 31 | ((o >> 5) & 0x3F) << 25 | rs2 << 20 | rs1 << 15 | f3 << 12 | ((o >> 1) & 0xF) << 8 | ((o >> 11) & 1) << 7 | 0x63
}
fn j_type(off: i32, rd: u32) -> u32 {
    let o = off as u32;
    ((o >> 20) & 1) << 31 | ((o >> 1) & 0x3FF) << 21 | ((o >> 11) & 1) << 20 | ((o >> 12) & 0xFF) << 12 | rd << 7 | 0x6F
}
fn lui(rd: u32, v: u32) -> u32 {
    (v & 0xFFFF_F000) | rd << 7 | 0x37
}
fn addi(rd: u32, rs: u32, imm: i32) -> u32 {
    i_type(imm, rs, 0, rd, 0x13)
}
fn lw(rd: u32, rs: u32, imm: i32) -> u32 {
    i_type(imm, rs, 2, rd, 0x03)
}
fn sw(rs2: u32, rs1: u32, imm: i32) -> u32 {
    s_type(imm, rs2, rs1, 2)
}
fn sb(rs2: u32, rs1: u32, imm: i32) -> u32 {
    s_type(imm, rs2, rs1, 0)
}
fn r_type(f7: u32, rs2: u32, rs1: u32, f3: u32, rd: u32) -> u32 {
    f7 << 25 | rs2 << 20 | rs1 << 15 | f3 << 12 | rd << 7 | 0x33
}
const ECALL: u32 = 0x0000_0073;

/// `lui` and `addi` that load `v` (the `addi`'s sign taken into account).
fn li(rd: u32, v: u32) -> [u32; 2] {
    let lo = ((v & 0xFFF) as i32) << 20 >> 20;
    [lui(rd, v.wrapping_sub(lo as u32)), addi(rd, rd, lo)]
}

/// Word indexes of the two `li` pairs `patch` fills in.
const END_AT: usize = 2;
const TABLE_AT: usize = 10;

fn code() -> Vec<u8> {
    let mut w: Vec<u32> = Vec::new();
    // Copy the ROM to RAM.
    w.push(lui(T0, ROM));
    w.push(lui(T1, RAM));
    w.extend(li(T2, 0)); // ROM's end (patched)
    let copy = w.len();
    w.push(lw(T3, T0, 0));
    w.push(sw(T3, T1, 0));
    w.push(addi(T0, T0, 4));
    w.push(addi(T1, T1, 4));
    w.push(b_type((copy as i32 - w.len() as i32) * 4, T2, T0, 6)); // bltu t0, t2
    assert_eq!(w.len() + 1, TABLE_AT);
    w.push(addi(S1, 0, 0));
    // Each frame.
    let frame = w.len();
    w.extend(li(S2, 0)); // the table in RAM (patched)
    w.push(lw(T0, S2, 0));
    w.push(r_type(1, T0, S1, 7, T0)); // remu t0, s1, t0
    w.push(i_type(2, T0, 1, T0, 0x13)); // slli t0, t0, 2
    w.push(r_type(0, S2, T0, 0, T0)); // add t0, t0, s2
    w.push(lw(T0, T0, 4));
    let entry = w.len();
    w.push(lw(A7, T0, 0));
    let to_done = w.len();
    w.push(0); // beqz a7, done
    w.push(lw(T1, T0, 16));
    w.push(b_type(4 * 4, 0, T1, 0)); // beqz t1, +4 words
    w.push(i_type(4, S1, 1, T2, 0x13)); // slli t2, s1, 4
    w.push(i_type(0xF0, T2, 7, T2, 0x13)); // andi t2, t2, F0h
    w.push(sb(T2, T1, 0));
    w.push(lw(A0, T0, 4));
    w.push(lw(A1, T0, 8));
    w.push(lw(A2, T0, 12));
    w.push(lw(A3, T0, 20));
    w.push(ECALL);
    w.push(addi(T0, T0, 24));
    w.push(j_type((entry as i32 - w.len() as i32) * 4, 0));
    let done = w.len();
    w[to_done] = b_type((done as i32 - to_done as i32) * 4, 0, A7, 0);
    w.push(addi(S1, S1, 1));
    w.push(addi(A7, 0, 0x100));
    w.push(ECALL);
    w.push(j_type((frame as i32 - w.len() as i32) * 4, 0));
    w.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn patch(rom: &mut [u8], rom_len: u32, table: u32) {
    let mut put = |i: usize, pair: [u32; 2]| {
        rom[4 * i..4 * i + 4].copy_from_slice(&pair[0].to_le_bytes());
        rom[4 * i + 4..4 * i + 8].copy_from_slice(&pair[1].to_le_bytes());
    };
    put(END_AT, li(T2, ROM + rom_len));
    put(TABLE_AT, li(S2, table));
}

// ------------------------------------------------------------ list helpers

/// A display list being written (SPEC §vector.list, §vector3d.list).
#[derive(Default, Clone)]
pub struct List(pub Vec<u8>);

impl List {
    pub fn op(&mut self, op: u8) -> &mut Self {
        self.0.push(op);
        self
    }
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn i16(&mut self, v: i16) -> &mut Self {
        self.u16(v as u16)
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Room left for `n` more bytes within `max`.
    pub fn room(&self, n: usize, max: usize) -> bool {
        self.0.len() + n <= max
    }
}

/// A deterministic sequence (xorshift32).
pub struct Rng(pub u32);

impl Rng {
    pub fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
}

// ------------------------------------------------------------ host work

/// What a machine's displays ask of a host in one frame (PERFORMANCE.md
/// R10), before the ink caps: the 2D list's ink in filled shapes and in
/// strokes (SPEC §vector.ink-cap), and the 3D list's counts (SPEC
/// §vector3d.caps) without its ink cap (its other caps applied), and
/// whether one of those stopped it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Work {
    pub fill: u64,
    pub stroke: u64,
    pub ink3: f64,
    pub length3: f64,
    pub segments3: usize,
    pub faces3: usize,
    pub capped3: bool,
}

impl Work {
    /// The work of `m`'s displays as they are now.
    /// `screens`: the handles whose Screens are in effect in its 3D list
    /// (a shell's, SPEC §vector3d.list), bit h − 1 for handle h.
    pub fn of(m: &crate::Machine, screens: u8) -> Work {
        let mut w = Work::default();
        if let Some(v) = m.vector() {
            (w.fill, w.stroke) = crate::vector::work(v.list(), v.outlined());
        }
        if let Some(v3) = m.vector3() {
            let p = crate::vector3::counts(v3.list(), screens);
            (w.ink3, w.length3, w.segments3, w.faces3, w.capped3) = (p.ink, p.length, p.counted, p.faces, p.capped);
        }
        w
    }

    /// The larger of each count.
    pub fn max(self, o: Work) -> Work {
        Work {
            fill: self.fill.max(o.fill),
            stroke: self.stroke.max(o.stroke),
            ink3: self.ink3.max(o.ink3),
            length3: self.length3.max(o.length3),
            segments3: self.segments3.max(o.segments3),
            faces3: self.faces3.max(o.faces3),
            capped3: self.capped3 || o.capped3,
        }
    }
}

impl std::fmt::Display for Work {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "2d fill {} stroke {}; 3d ink {:.0} length {:.0} segments {} faces {}{}",
            self.fill,
            self.stroke,
            self.ink3,
            self.length3,
            self.segments3,
            self.faces3,
            if self.capped3 { " (capped)" } else { "" }
        )
    }
}

/// What a scenario's image asks of the vector displays after `frames`
/// frames and what the caps let a host draw of it (SPEC §vector.ink-cap,
/// §vector3d.caps): `(asked, drawn, cap)` for the 2D list and for the 3D
/// list's ink, in square screen units, or `None` for a display the image
/// does not use.
pub struct Enforced {
    pub ink2: Option<(u64, u64, u64)>,
    pub ink3: Option<(f64, f64, f64)>,
}

impl Enforced {
    /// Runs `image` headless for `frames` frames and measures its lists.
    pub fn of(image: &[u8], frames: u32) -> Result<Enforced, String> {
        let image = Image::parse(image).map_err(|e| e.to_string())?;
        let mut m = crate::Machine::new(&image);
        for _ in 0..frames {
            m.run_frame(&crate::FrameInput::default());
        }
        if let Some(s) = m.stopped() {
            return Err(format!("stopped: {s:?}"));
        }
        let ink2 = m.vector().filter(|v| !v.list().is_empty()).map(|v| {
            let (f, s) = crate::vector::work(v.list(), v.outlined());
            (f + s, v.shapes().map(|p| crate::vector::ink(&p)).sum(), crate::vector::INK_CAP)
        });
        let ink3 = m.vector3().filter(|v| !v.list().is_empty()).map(|v| {
            let asked = crate::vector3::counts(v.list(), 0).ink;
            let drawn = crate::vector3::project(v.list(), (0.0, 0.0), &mut Vec::new()).ink;
            (asked, drawn, crate::vector3::INK_CAP)
        });
        Ok(Enforced { ink2, ink3 })
    }

    /// Whether what is drawn is within the caps.
    pub fn within(&self) -> bool {
        self.ink2.is_none_or(|(_, d, c)| d <= c) && self.ink3.is_none_or(|(_, d, c)| d <= c)
    }
}

impl std::fmt::Display for Enforced {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if let Some((a, d, c)) = self.ink2 {
            parts.push(format!("2D ink {d} of {c} (asks {a})"));
        }
        if let Some((a, d, c)) = self.ink3 {
            parts.push(format!("3D ink {d:.0} of {c:.0} (asks {a:.0})"));
        }
        if parts.is_empty() {
            parts.push("no vector list".into());
        }
        write!(f, "{}", parts.join("; "))
    }
}

// ------------------------------------------------------------ scenarios

/// A scenario: an image and what it asks of the host.
pub struct Scenario {
    pub id: &'static str,
    pub what: &'static str,
    pub image: Vec<u8>,
}

const V_SUBMIT: u32 = 0x300;
const V_STYLE: u32 = 0x302;
const V_FILL: u32 = 0x304;
const V3_SUBMIT: u32 = 0x900;
const V3_STYLE: u32 = 0x902;

fn d(slot: u8) -> DeviceReq {
    DeviceReq::new(slot, 0)
}

/// An image that submits `list` to device 3 every frame (its x origin
/// moved), after `setup` calls.
fn vector_image(title: &str, list: &List, setup: &[Call]) -> Vec<u8> {
    let mut b = Builder::new();
    let l = b.blob(&list.0);
    let mut calls = setup.to_vec();
    calls.push(Call { a7: V_SUBMIT, args: [l, list.len() as u32, 0, 0], poke: at(l, 1) });
    b.frame(calls);
    b.build(title, &[d(2), d(3)])
}

/// The same for device 9.
fn vector3_image(title: &str, list: &List, setup: &[Call]) -> Vec<u8> {
    let mut b = Builder::new();
    let l = b.blob(&list.0);
    let mut calls = setup.to_vec();
    calls.push(Call { a7: V3_SUBMIT, args: [l, list.len() as u32, 0, 0], poke: at(l, 1) });
    b.frame(calls);
    b.build(title, &[d(2), d(9)])
}

/// A 2D list that starts with an Origin (moved every frame) and a colour.
fn list2(rgba: [u8; 4]) -> List {
    let mut l = List::default();
    l.op(0x03).u16(0).u16(0);
    l.op(0x01).u8(rgba[0]).u8(rgba[1]).u8(rgba[2]).u8(rgba[3]);
    l
}

/// 3,267 lines of the greatest width (255) zig-zagging across the screen
/// (programs/conform's list B without its fills).
pub fn lines_2d() -> List {
    let mut l = list2([0x40, 0xA0, 0xFF, 0x20]);
    l.op(0x02).u8(255);
    let mut k = 0u32;
    while l.room(5, 16_384) {
        l.op(0x05).u16(if k & 1 == 1 { 0 } else { 10_240 }).u16(((k * 37) % 7681) as u16);
        k += 1;
    }
    l
}

/// Lines `width` sixteenths wide zig-zagging across the screen, as many
/// as a list holds.
pub fn thin_lines_2d(width: u8) -> List {
    let mut l = list2([0xFF, 0xFF, 0xFF, 0xFF]);
    l.op(0x02).u8(width);
    let mut k = 0u32;
    while l.room(5, 16_384) {
        l.op(0x05).u16(if k & 1 == 1 { 0 } else { 10_240 }).u16(((k * 37) % 7681) as u16);
        k += 1;
    }
    l
}

/// Circle outlines of the greatest width, 3 bytes each, radii 40 to 300
/// units about the centre of the screen.
pub fn circles_2d() -> List {
    let mut l = list2([0xFF, 0x80, 0x40, 0x20]);
    l.op(0x02).u8(255).op(0x04).u16(5120).u16(3840);
    let mut k = 0u32;
    while l.room(3, 16_384) {
        l.op(0x06).u16((16 * (40 + k % 261)) as u16);
        k += 1;
    }
    l
}

/// Translucent rectangles over the left half of the screen (76,800 of ink
/// each): the ink cap (SPEC §vector.ink-cap) lets 3 through, exactly the cap.
pub fn rects_2d() -> List {
    let mut l = list2([0x20, 0xFF, 0x60, 0x30]);
    while l.room(10, 16_384) {
        l.op(0x04).u16(0).u16(0).op(0x08).u16(5120).u16(7680);
    }
    l
}

/// Translucent discs of radius 240 units at the centre (115,200 of ink
/// each): the ink cap lets 2 through, exactly the cap.
pub fn discs_2d() -> List {
    let mut l = list2([0x20, 0xFF, 0x60, 0x30]);
    l.op(0x04).u16(5120).u16(3840);
    while l.room(3, 16_384) {
        l.op(0x07).u16(3840);
    }
    l
}

/// Rectangles over the whole screen as outlines (fill mode 1): four lines
/// of the greatest width each.
pub fn rect_outlines_2d() -> List {
    let mut l = list2([0xFF, 0xFF, 0x60, 0x30]);
    l.op(0x02).u8(255).op(0x04).u16(0).u16(0);
    let mut k = 0u16;
    while l.room(5, 16_384) {
        l.op(0x08).u16(10_240 - (k % 64) * 16).u16(7680 - (k % 48) * 16);
        k += 1;
    }
    l
}

/// The most shapes a list can hold of a kind that draws: 2,047 small
/// circles, one unit wide, each at its own place.
pub fn many_2d() -> List {
    let mut l = list2([0xFF, 0xFF, 0xFF, 0xFF]);
    l.op(0x02).u8(16);
    let mut k = 0u32;
    while l.room(8, 16_384) {
        l.op(0x04).u16((k * 97 % 10_240) as u16).u16((k * 61 % 7680) as u16);
        l.op(0x06).u16(16);
        k += 1;
    }
    l
}

/// A 3D list that starts with a Translate (moved every frame), a colour
/// and a width.
fn list3(rgba: [u8; 4], width: u16) -> List {
    let mut l = List::default();
    l.op(0x09).u16(0).u16(0).u16(0);
    l.op(0x01).u8(rgba[0]).u8(rgba[1]).u8(rgba[2]).u8(rgba[3]);
    l.op(0x02).u16(width);
    l
}

/// 3D lines of the greatest width on the glass across the screen, up to
/// the ink cap.
pub fn lines_3d() -> List {
    let mut l = list3([0x40, 0xFF, 0x80, 0x40], 255);
    let mut k = 0i32;
    l.op(0x03).i16(0).i16(0).i16(0);
    while l.room(7, 16_384) {
        let y = (k * 37 % 480) as i16 * 16;
        l.op(0x04).i16(if k & 1 == 1 { 0 } else { 10_239 }).i16(y).i16(0);
        k += 1;
    }
    l
}

/// 16,384 points of the greatest width (the segment cap), on the glass.
pub fn points_3d() -> List {
    let mut l = list3([0xFF, 0x80, 0x40, 0x40], 255);
    let mut k = 0i32;
    while l.room(8, 16_384) {
        l.op(0x03).i16((k * 97 % 640) as i16 * 16).i16((k * 61 % 480) as i16 * 16).i16(0);
        l.op(0x05);
        k += 1;
    }
    l
}

/// Hidden lines: `faces` quads standing in front of a grid of lines, as
/// many segments as faces × segments ≤ 131,072 allows.
pub fn hidden_3d(faces: usize) -> List {
    let mut l = list3([0xFF, 0xFF, 0xFF, 0xFF], 16);
    let mut r = Rng(0x1234_5678);
    for _ in 0..faces {
        let (x, y) = ((r.next() % 600) as i16 * 16, (r.next() % 440) as i16 * 16);
        let z = (r.next() % 400) as i16 * 16;
        let s = 40 * 16;
        l.op(0x0C).u8(4);
        for (cx, cy) in [(0, 0), (s, 0), (s, s), (0, s)] {
            l.i16(x + cx).i16(y + cy).i16(z);
        }
    }
    let segments = (131_072 / faces.max(1)).min(16_384);
    let mut n = 0;
    while n < segments && l.room(14, 16_384) {
        let y = (n % 480) as i16 * 16;
        l.op(0x03).i16(0).i16(y).i16(8000).op(0x04).i16(10_239).i16(y).i16(8000);
        n += 1;
    }
    l
}

/// Four Surfaces of the whole screen, each on a panel 210 units wide (an
/// eighth of the screen), tilted, near the glass: up to the 3D ink cap.
pub fn surfaces_3d() -> List {
    let mut l = list3([0xFF, 0xFF, 0xFF, 0xFF], 16);
    for k in 0..4u16 {
        l.op(0x07);
        l.op(0x09).i16(5120).i16(3840).i16(16 * 40 * k as i16);
        l.op(0x0A).u8(1).u16(2000 + 1000 * k);
        l.op(0x0E).u16(0).u16(0).u16(640).u16(480).u16(16 * 210);
        l.op(0x08);
    }
    l
}

/// The scenarios of today's caps, device by device.
pub fn scenarios() -> Vec<Scenario> {
    let phosphor = [Call::new(V_STYLE, [1, 0, 0, 0])];
    let outlines = [Call::new(V_FILL, [1, 0, 0, 0])];
    let phosphor3 = [Call::new(V3_STYLE, [1, 0, 0, 0])];
    let mut out = vec![
        Scenario { id: "2d-lines", what: "2D: 3,267 lines 16 units wide across the screen", image: vector_image("w-lines", &lines_2d(), &[]) },
        Scenario { id: "2d-thin-lines", what: "2D: lines 2 units wide across the screen", image: vector_image("w-thin", &thin_lines_2d(32), &[]) },
        Scenario { id: "2d-hair-lines", what: "2D: lines 1/16 unit wide across the screen", image: vector_image("w-hair", &thin_lines_2d(1), &[]) },
        Scenario { id: "2d-circles", what: "2D: 5,454 circles 16 units wide, radius 40-300", image: vector_image("w-circles", &circles_2d(), &[]) },
        Scenario { id: "2d-rects", what: "2D: translucent half-screen rectangles up to the ink cap", image: vector_image("w-rects", &rects_2d(), &[]) },
        Scenario { id: "2d-discs", what: "2D: translucent discs up to the ink cap", image: vector_image("w-discs", &discs_2d(), &[]) },
        Scenario { id: "2d-rect-outlines", what: "2D fill mode 1: 3,275 screen-sized rectangles as 16-unit outlines", image: vector_image("w-outlines", &rect_outlines_2d(), &outlines) },
        Scenario { id: "2d-many", what: "2D: 2,047 small circles", image: vector_image("w-many", &many_2d(), &[]) },
        Scenario { id: "2d-lines-phosphor", what: "2D phosphor: the lines", image: vector_image("w-lines-ph", &lines_2d(), &phosphor) },
        Scenario { id: "2d-circles-phosphor", what: "2D phosphor: the circles", image: vector_image("w-circles-ph", &circles_2d(), &phosphor) },
        Scenario { id: "3d-lines", what: "3D: lines 16 units wide up to the ink cap", image: vector3_image("w3-lines", &lines_3d(), &[]) },
        Scenario { id: "3d-points", what: "3D: 16,384 points 16 units wide", image: vector3_image("w3-points", &points_3d(), &[]) },
        Scenario { id: "3d-lines-phosphor", what: "3D phosphor: the lines", image: vector3_image("w3-lines-ph", &lines_3d(), &phosphor3) },
        Scenario { id: "3d-points-phosphor", what: "3D phosphor: the points", image: vector3_image("w3-points-ph", &points_3d(), &phosphor3) },
        Scenario { id: "3d-hidden-256", what: "3D: 256 faces x 512 segments", image: vector3_image("w3-hid256", &hidden_3d(256), &[]) },
        Scenario { id: "3d-hidden-8", what: "3D: 8 faces x 16,384 segments", image: vector3_image("w3-hid8", &hidden_3d(8), &[]) },
        Scenario { id: "3d-surfaces", what: "3D: 4 Surfaces of the whole screen, each an eighth of the screen", image: vector3_image("w3-surf", &surfaces_3d(), &[]) },
    ];
    out.extend(terminal_scenarios());
    out.extend(page_scenarios());
    out.push(Scenario { id: "3d-points-faces", what: "3D: points behind 256 faces", image: vector3_image("w3-ptsf", &points_behind_faces_3d(256), &[]) });
    out.push(combined_scenario());
    out
}

/// The terminal rewritten every frame: basic text, and (device 10) CJK
/// characters not shown before, 1,200 a frame for 30 frames.
fn terminal_scenarios() -> Vec<Scenario> {
    let mut out = Vec::new();
    let mut b = Builder::new();
    for f in 0..8u32 {
        let text: Vec<u8> = (0..2400u32).map(|i| b'!' + ((i + f) % 90) as u8).collect();
        let t = b.blob(&text);
        b.frame(vec![
            Call::new(0x202, [0, 0, 0, 0]),
            Call::new(0x204, [9 + f, 232 + f, 0, 0]),
            Call::new(0x200, [t, 2400, 0, 0]),
        ]);
    }
    out.push(Scenario { id: "term-basic", what: "terminal: every cell rewritten each frame, basic text", image: b.build("w-term", &[d(2)]) });
    let mut b = Builder::new();
    let mut c = 0x4E00u32;
    for f in 0..30u32 {
        let mut text = Vec::new();
        for _ in 0..1200 {
            text.extend_from_slice(char::from_u32(c).unwrap().encode_utf8(&mut [0; 4]).as_bytes());
            // CJK Unified Ideographs, then Extension A, then Hangul.
            c = match c + 1 {
                0xA000 => 0x3400,
                0x4DC0 => 0xAC00,
                n => n,
            };
        }
        let t = b.blob(&text);
        b.frame(vec![
            Call::new(0x202, [0, 0, 0, 0]),
            Call::new(0x204, [9 + f % 7, 232 + f % 24, 0, 0]),
            Call::new(0x200, [t, text.len() as u32, 0, 0]),
        ]);
    }
    out.push(Scenario {
        id: "term-cjk",
        what: "terminal, full text: 1,200 CJK characters not shown before, each frame",
        image: b.build("w-cjk", &[d(2), d(10)]),
    });
    out
}

/// The page: a document of the smallest text, scrolled every frame.
fn page_scenarios() -> Vec<Scenario> {
    let doc = page_doc();
    let mut b = Builder::new();
    let p = b.blob(&doc);
    b.frame(vec![Call::new(0x702, [0, 0, 640, 480]), Call::new(0x700, [p, doc.len() as u32, 0, 0])]);
    for f in 1..60u32 {
        b.frame(vec![Call::new(0x703, [f * 37, 0, 0, 0])]);
    }
    vec![Scenario {
        id: "page-glyphs",
        what: "page: size-6 text filling the rectangle, scrolled each frame",
        image: b.build("w-page", &[d(2), d(7)]),
    }]
}

/// A document of 65,536 bytes of words in the smallest text (size 6).
pub fn page_doc() -> Vec<u8> {
    let mut doc = vec![0x03, 6]; // size 6
    let mut r = Rng(99);
    while doc.len() < 65_000 {
        let mut t = Vec::new();
        while t.len() < 4000 {
            let w = 1 + r.next() % 9;
            for _ in 0..w {
                t.push(b'a' + (r.next() % 26) as u8);
            }
            t.push(b' ');
        }
        doc.push(0x01);
        doc.extend_from_slice(&(t.len() as u16).to_le_bytes());
        doc.extend_from_slice(&t);
    }
    doc.truncate(65_536);
    doc
}

/// Points of the greatest width on the glass and, behind them, `faces`
/// faces that hide none of them but are each tested against every point.
pub fn points_behind_faces_3d(faces: usize) -> List {
    let mut l = list3([0xFF, 0x80, 0x40, 0x40], 255);
    let mut r = Rng(77);
    for _ in 0..faces {
        let (x, y) = ((r.next() % 600) as i16 * 16, (r.next() % 440) as i16 * 16);
        l.op(0x0C).u8(4);
        for (cx, cy) in [(0, 0), (640, 0), (640, 640), (0, 640)] {
            l.i16(x + cx).i16(y + cy).i16(32_000);
        }
    }
    let mut k = 0i32;
    while l.room(8, 16_384) {
        l.op(0x03).i16((k * 97 % 640) as i16 * 16).i16((k * 61 % 480) as i16 * 16).i16(0);
        l.op(0x05);
        k += 1;
    }
    l
}

/// Every device at once, each at its worst: the terminal rewritten, the
/// page's smallest text scrolled, rectangles up to the 2D ink cap (the costliest per unit), and points
/// up to the 3D caps behind 256 faces.
fn combined_scenario() -> Scenario {
    let mut b = Builder::new();
    let doc = page_doc();
    let p = b.blob(&doc);
    let l2 = rects_2d();
    let v2 = b.blob(&l2.0);
    let l3 = points_behind_faces_3d(256);
    let v3 = b.blob(&l3.0);
    for f in 0..8u32 {
        let text: Vec<u8> = (0..2400u32).map(|i| b'!' + ((i + f) % 90) as u8).collect();
        let t = b.blob(&text);
        let mut calls = Vec::new();
        if f == 0 {
            calls.push(Call::new(0x702, [0, 0, 640, 480]));
            calls.push(Call::new(0x700, [p, doc.len() as u32, 0, 0]));
        }
        calls.extend([
            Call::new(0x703, [f * 37, 0, 0, 0]),
            Call::new(0x202, [0, 0, 0, 0]),
            Call::new(0x204, [9 + f, 232 + f, 0, 0]),
            Call::new(0x200, [t, 2400, 0, 0]),
            Call { a7: V_SUBMIT, args: [v2, l2.len() as u32, 0, 0], poke: at(v2, 1) },
            Call { a7: V3_SUBMIT, args: [v3, l3.len() as u32, 0, 0], poke: at(v3, 1) },
        ]);
        b.frame(calls);
    }
    Scenario {
        id: "all",
        what: "every device at its worst at once: terminal, page, 2D rectangles, 3D points behind faces",
        image: b.build("w-all", &[d(2), d(3), d(7), d(9)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameInput, Machine};

    /// The program copies its data and makes its calls every frame.
    #[test]
    fn the_program_makes_its_calls() {
        let mut b = Builder::new();
        let text = b.blob(b"hello");
        let other = b.blob(b"world!");
        b.frame(vec![Call::new(0x200, [text, 5, 0, 0])]);
        b.frame(vec![Call::new(0x202, [0, 1, 0, 0]), Call::new(0x200, [other, 6, 0, 0])]);
        let file = b.build("t", &[DeviceReq::new(2, 0)]);
        let image = Image::parse(&file).unwrap();
        let mut m = Machine::new(&image);
        for _ in 0..3 {
            m.run_frame(&FrameInput::default());
        }
        assert!(m.stopped().is_none(), "{:?}", m.stopped());
        let t = m.terminal().unwrap();
        let row = |r: usize| (0..12).map(|c| char::from_u32(t.cell(c, r).ch as u32).unwrap()).collect::<String>();
        assert_eq!(row(0), "hello       ");
        assert_eq!(row(1), "world!hello ");
    }
}
