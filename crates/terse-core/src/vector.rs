//! Device 3, 2D vector display (SPEC §vector).
//!
//! The device only validates and copies the display list. [`decode`] turns a
//! stored list into resolved primitives (origin applied, drawing state
//! tracked, SPEC §vector.list termination rules, the ink cap of SPEC §vector.ink-cap); hosts
//! and tests draw from it rather than parsing the list themselves.

use std::fmt;

/// Longest list a submit accepts (SPEC §vector.calls).
pub const MAX_LIST: usize = 16_384;

/// Results of `submit` (and of `style`, `persistence`, `fill`).
pub const RESULT_DONE: u32 = 0;
pub const RESULT_REJECTED: u32 = 1;
/// Styles (SPEC §vector.phosphor).
pub const STYLE_CLEAN: u8 = 0x00;
pub const STYLE_PHOSPHOR: u8 = 0x01;
/// Persistence after reset (SPEC §vector.calls).
pub const DEFAULT_PERSISTENCE: u8 = 160;
/// Most ink, in square screen units, that one list may draw (SPEC
/// §vector.ink-cap): three quarters of the screen. Provisional.
pub const INK_CAP: u64 = 3 * 640 * 480 / 4;
/// The screen in sixteenths of a screen unit (SPEC §vector.coordinates).
const SCREEN_W16: i32 = 640 * 16;
const SCREEN_H16: i32 = 480 * 16;

/// Display-list opcodes (SPEC §vector.list).
pub mod op {
    pub const END: u8 = 0x00;
    pub const COLOUR: u8 = 0x01;
    pub const WIDTH: u8 = 0x02;
    pub const ORIGIN: u8 = 0x03;
    pub const MOVE: u8 = 0x04;
    pub const LINE: u8 = 0x05;
    pub const CIRCLE: u8 = 0x06;
    pub const DISC: u8 = 0x07;
    pub const RECT: u8 = 0x08;
    pub const TRIANGLE: u8 = 0x09;
}

/// The device state (SPEC §vector.determinism): style, persistence, fill mode and the
/// list copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vector {
    style: u8,
    persistence: u8,
    /// False after a `fill(1)`: filled shapes are drawn as outlines.
    fill: bool,
    list: Vec<u8>,
    /// Changes whenever the list's contents change (not part of the
    /// machine state; for hosts that cache what they upload).
    generation: u64,
}

impl Default for Vector {
    /// The state after reset (SPEC §vector.calls).
    fn default() -> Self {
        Vector { style: STYLE_CLEAN, persistence: DEFAULT_PERSISTENCE, fill: true, list: Vec::new(), generation: 0 }
    }
}

impl Vector {
    /// Replaces the list with `bytes` if it is 1 to 16,384 bytes long;
    /// returns [`RESULT_DONE`] or [`RESULT_REJECTED`] (the list is kept).
    /// The caller checks that the bytes lie in RAM.
    pub fn submit(&mut self, bytes: &[u8]) -> u32 {
        if bytes.is_empty() || bytes.len() > MAX_LIST {
            return RESULT_REJECTED;
        }
        if self.list != bytes {
            self.list.clear();
            self.list.extend_from_slice(bytes);
            self.generation += 1;
        }
        RESULT_DONE
    }

    /// Empties the list.
    pub fn clear(&mut self) {
        if !self.list.is_empty() {
            self.list.clear();
            self.generation += 1;
        }
    }

    /// Sets the style: 0 clean, 1 phosphor; anything else is rejected.
    pub fn set_style(&mut self, n: u32) -> u32 {
        match n {
            0 | 1 => {
                self.style = n as u8;
                RESULT_DONE
            }
            _ => RESULT_REJECTED,
        }
    }

    /// Sets the persistence, 0-255; anything else is rejected.
    pub fn set_persistence(&mut self, n: u32) -> u32 {
        match u8::try_from(n) {
            Ok(p) => {
                self.persistence = p;
                RESULT_DONE
            }
            Err(_) => RESULT_REJECTED,
        }
    }

    /// Sets the fill mode: 0 filled shapes are filled, 1 they are drawn as
    /// outlines; anything else is rejected.
    pub fn set_fill(&mut self, n: u32) -> u32 {
        match n {
            0 | 1 => {
                self.fill = n == 0;
                RESULT_DONE
            }
            _ => RESULT_REJECTED,
        }
    }

    /// The list the device holds (the bytes of the last successful submit).
    pub fn list(&self) -> &[u8] {
        &self.list
    }

    /// The style: [`STYLE_CLEAN`] or [`STYLE_PHOSPHOR`].
    pub fn style(&self) -> u8 {
        self.style
    }

    /// True in the phosphor style (SPEC §vector.phosphor).
    pub fn phosphor(&self) -> bool {
        self.style == STYLE_PHOSPHOR
    }

    /// The persistence, 0-255 (SPEC §vector.phosphor).
    pub fn persistence(&self) -> u8 {
        self.persistence
    }

    /// Whether filled shapes are filled (false after `fill(1)`).
    pub fn fill(&self) -> bool {
        self.fill
    }

    /// Whether shapes are drawn as outlines: in the phosphor style, or with
    /// fill off.
    pub fn outlined(&self) -> bool {
        self.phosphor() || !self.fill
    }

    /// The line `terse run --dump-vectors` prints before the list when the
    /// style or fill mode is not the default, e.g.
    /// `-- vector style: phosphor, persistence 160 --` or
    /// `-- vector style: clean, fill off --`.
    pub fn style_line(&self) -> Option<String> {
        match (self.phosphor(), self.fill) {
            (false, true) => None,
            (false, false) => Some("-- vector style: clean, fill off --".into()),
            (true, true) => Some(format!("-- vector style: phosphor, persistence {} --", self.persistence)),
            (true, false) => Some(format!("-- vector style: phosphor, persistence {}, fill off --", self.persistence)),
        }
    }

    /// Style, persistence, fill mode and list (STATE.md, vector section);
    /// not the generation counter.
    pub fn write_state(&self, out: &mut Vec<u8>) {
        out.push(self.style);
        out.push(self.persistence);
        out.push(!self.fill as u8);
        out.extend_from_slice(&(self.list.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.list);
    }

    /// The vector section as `write_state` wrote it (STATE.md 4).
    pub fn restore(&mut self, r: &mut crate::snapshot::Reader) -> Result<(), crate::snapshot::RestoreError> {
        use crate::snapshot::bad;
        let [style, persistence, outlines] = r.array()?;
        if style > STYLE_PHOSPHOR || outlines > 1 {
            return Err(bad("vector style or fill mode"));
        }
        let m = r.u32()? as usize;
        if m > MAX_LIST {
            return Err(bad("display list length"));
        }
        self.style = style;
        self.persistence = persistence;
        self.fill = outlines == 0;
        self.list = r.take(m)?.to_vec();
        self.generation = !self.list.is_empty() as u64;
        Ok(())
    }

    /// A counter that changes whenever [`Vector::list`] changes.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// What the host draws: the outlines of the list when [`Vector::outlined`],
    /// else its primitives, each within the ink cap.
    pub fn shapes(&self) -> Box<dyn Iterator<Item = Primitive> + '_> {
        if self.outlined() { Box::new(decode_outlines(&self.list)) } else { Box::new(decode(&self.list)) }
    }

    /// [`Vector::shapes`] as text, one per line (`terse run --dump-vectors`).
    pub fn dump(&self) -> String {
        self.shapes().map(|p| format!("{p}\n")).collect()
    }
}

/// Colour as (r, g, b, a); `a` is opacity, 255 opaque.
pub type Rgba = [u8; 4];

/// One drawable shape with every coordinate resolved (origin added, wrapped
/// to 16 bits), in sixteenths of a screen unit. `width` is the effective
/// width in sixteenths (a width of 0 is already 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primitive {
    Line { x0: i16, y0: i16, x1: i16, y1: i16, width: u8, rgba: Rgba },
    Circle { x: i16, y: i16, radius: u16, width: u8, rgba: Rgba },
    Disc { x: i16, y: i16, radius: u16, rgba: Rgba },
    Rect { x0: i16, y0: i16, x1: i16, y1: i16, rgba: Rgba },
    Triangle { x0: i16, y0: i16, x1: i16, y1: i16, x2: i16, y2: i16, rgba: Rgba },
    /// A segment of the 3D vector display as seen on the glass
    /// (`crate::vector3::project`): drawn like a line, with its opacity
    /// times `fade` (255 = 1) at each end, interpolated along it; an end
    /// whose bit is set in `butt` (bit 0 the start, bit 1 the end) has no
    /// round cap and no vertex dwell, where a circle's chords join.
    Beam { x0: i16, y0: i16, x1: i16, y1: i16, width: u8, rgba: Rgba, fade: [u8; 2], butt: u8 },
}

fn hex(rgba: &Rgba) -> String {
    rgba.iter().map(|b| format!("{b:02x}")).collect()
}

/// A stable one-line text form, used by `terse run --dump-vectors`:
/// `line x0 y0 x1 y1 w <width> c <rrggbbaa>` and so on.
impl fmt::Display for Primitive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Primitive::Line { x0, y0, x1, y1, width, rgba } => {
                write!(f, "line {x0} {y0} {x1} {y1} w {width} c {}", hex(rgba))
            }
            Primitive::Circle { x, y, radius, width, rgba } => {
                write!(f, "circle {x} {y} r {radius} w {width} c {}", hex(rgba))
            }
            Primitive::Disc { x, y, radius, rgba } => write!(f, "disc {x} {y} r {radius} c {}", hex(rgba)),
            Primitive::Rect { x0, y0, x1, y1, rgba } => write!(f, "rect {x0} {y0} {x1} {y1} c {}", hex(rgba)),
            Primitive::Triangle { x0, y0, x1, y1, x2, y2, rgba } => {
                write!(f, "triangle {x0} {y0} {x1} {y1} {x2} {y2} c {}", hex(rgba))
            }
            Primitive::Beam { x0, y0, x1, y1, width, rgba, fade, butt } => {
                write!(f, "beam {x0} {y0} {x1} {y1} w {width} c {} f {} {} b {butt}", hex(rgba), fade[0], fade[1])
            }
        }
    }
}

/// Decodes a display list (SPEC §vector.list) as the clean style with fill
/// mode 0 draws it: up to the shape that the ink cap (SPEC §vector.ink-cap)
/// stops.
pub fn decode(list: &[u8]) -> Decoder<'_> {
    decode_within(list, INK_CAP)
}

/// [`decode`] with the ink cap lowered to `cap` (at most [`INK_CAP`]): a
/// machine's share of a system's ink (SPEC §machines.ink).
pub fn decode_within(list: &[u8], cap: u64) -> Decoder<'_> {
    let ink = Ink { off: false, total: 0, cap: cap.min(INK_CAP) };
    Decoder { list, at: 0, rgba: [255; 4], width: 16, origin: (0, 0), point: (0, 0), ink }
}

/// Decodes a display list without the ink cap (for outlines, where
/// [`Outlines`] applies it to the outlines it gives).
fn decode_uncapped(list: &[u8]) -> Decoder<'_> {
    Decoder { ink: Ink::OFF, ..decode(list) }
}

/// The ink cap's running count (SPEC §vector.ink-cap).
#[derive(Clone, Copy, Debug, Default)]
struct Ink {
    /// The cap does not apply (it is applied elsewhere).
    off: bool,
    /// Ink of the shapes drawn so far.
    total: u64,
    /// The cap: [`INK_CAP`], or less in a system (SPEC §machines.ink).
    cap: u64,
}

impl Ink {
    const OFF: Ink = Ink { off: true, total: 0, cap: INK_CAP };

    /// Whether `p` is drawn, counting it if so.
    fn admit(&mut self, p: &Primitive) -> bool {
        if self.off {
            return true;
        }
        let ink = ink(p);
        if self.total + ink > self.cap {
            return false;
        }
        self.total += ink;
        true
    }
}

/// The ink a shape counts against the ink cap (SPEC §vector.ink-cap), in
/// square screen units: [`fill_ink`] for a filled shape, [`stroke_ink`]
/// for a line or circle outline.
pub fn ink(p: &Primitive) -> u64 {
    fill_ink(p) + stroke_ink(p)
}

/// The ink of a filled shape (SPEC §vector.ink-cap): half its bounding box
/// (a disc's bounding square) clipped to the screen, in sixteenths
/// squared, divided by 512 and rounded up. Lines and circle outlines count
/// 0 here.
pub fn fill_ink(p: &Primitive) -> u64 {
    let (x0, y0, x1, y1) = match *p {
        Primitive::Disc { x, y, radius, .. } => {
            let (x, y, r) = (x as i32, y as i32, radius as i32);
            (x - r, y - r, x + r, y + r)
        }
        Primitive::Rect { x0, y0, x1, y1, .. } => {
            let (x0, y0, x1, y1) = (x0 as i32, y0 as i32, x1 as i32, y1 as i32);
            (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
        }
        Primitive::Triangle { x0, y0, x1, y1, x2, y2, .. } => {
            let xs = [x0 as i32, x1 as i32, x2 as i32];
            let ys = [y0 as i32, y1 as i32, y2 as i32];
            (*xs.iter().min().unwrap(), *ys.iter().min().unwrap(), *xs.iter().max().unwrap(), *ys.iter().max().unwrap())
        }
        Primitive::Line { .. } | Primitive::Circle { .. } | Primitive::Beam { .. } => return 0,
    };
    let w = (x1.min(SCREEN_W16) - x0.max(0)).max(0) as u64;
    let h = (y1.min(SCREEN_H16) - y0.max(0)).max(0) as u64;
    (w * h).div_ceil(512)
}

/// The ink of a line or circle outline (SPEC §vector.ink-cap), in square
/// screen units: its stroke, (length + width) × (width + 3 units) with a
/// line's length |dx| + |dy| and a circle's 8 × radius, or the box it can
/// reach on the screen if that is smaller, in sixteenths squared, divided
/// by 256 and rounded up. Filled shapes count 0 here; a 3D segment on the
/// glass ([`Primitive::Beam`]) counts as a line (for hosts that shed work
/// by it; the 3D display's own cap is SPEC §vector3d.caps).
pub fn stroke_ink(p: &Primitive) -> u64 {
    let (lo, hi, length, w) = match *p {
        Primitive::Line { x0, y0, x1, y1, width, .. } | Primitive::Beam { x0, y0, x1, y1, width, .. } => {
            let (x0, y0, x1, y1) = (x0 as i64, y0 as i64, x1 as i64, y1 as i64);
            ((x0.min(x1), y0.min(y1)), (x0.max(x1), y0.max(y1)), (x1 - x0).abs() + (y1 - y0).abs(), width as i64)
        }
        Primitive::Circle { x, y, radius, width, .. } => {
            let (x, y, r) = (x as i64, y as i64, radius as i64);
            ((x - r, y - r), (x + r, y + r), 8 * r, width as i64)
        }
        _ => return 0,
    };
    stroke_area(lo, hi, length, w).div_ceil(256)
}

/// min((length + w) × (w + 48), the box lo–hi grown by (w + 48) / 2 and
/// clipped to the screen), all in sixteenths: SPEC §vector.ink-cap's
/// count before the division.
fn stroke_area(lo: (i64, i64), hi: (i64, i64), length: i64, w: i64) -> u64 {
    let stroke = (length + w) * (w + 48);
    let g = (w + 48) / 2;
    let bw = ((hi.0 + g).min(SCREEN_W16 as i64) - (lo.0 - g).max(0)).max(0);
    let bh = ((hi.1 + g).min(SCREEN_H16 as i64) - (lo.1 - g).max(0)).max(0);
    stroke.min(bw * bh) as u64
}

/// Iterator over the primitives of a list; see [`decode`].
#[derive(Clone, Debug)]
pub struct Decoder<'a> {
    list: &'a [u8],
    at: usize,
    rgba: Rgba,
    width: u8,
    origin: (u16, u16),
    point: (i16, i16),
    /// The ink cap.
    ink: Ink,
}

/// Operand bytes of each opcode, or `None` for End and unknown opcodes.
fn operand_len(opcode: u8) -> Option<usize> {
    Some(match opcode {
        op::COLOUR => 4,
        op::WIDTH => 1,
        op::ORIGIN | op::MOVE | op::LINE | op::RECT => 4,
        op::CIRCLE | op::DISC => 2,
        op::TRIANGLE => 8,
        _ => return None,
    })
}

impl Decoder<'_> {
    /// Coordinate pair `i` of the operands `o`, origin added with 16-bit wrap.
    fn point(&self, o: &[u8], i: usize) -> (i16, i16) {
        let raw = |j: usize| u16::from_le_bytes([o[j], o[j + 1]]);
        (raw(4 * i).wrapping_add(self.origin.0) as i16, raw(4 * i + 2).wrapping_add(self.origin.1) as i16)
    }
}

impl Iterator for Decoder<'_> {
    type Item = Primitive;

    fn next(&mut self) -> Option<Primitive> {
        loop {
            let p = self.next_uncapped()?;
            if self.ink.admit(&p) {
                return Some(p);
            }
            // Over the cap: neither it nor anything after it is drawn.
            self.at = self.list.len();
            return None;
        }
    }
}

impl Decoder<'_> {
    fn next_uncapped(&mut self) -> Option<Primitive> {
        loop {
            let &opcode = self.list.get(self.at)?;
            // End, unknown opcode, or operands cut off: the list ends here.
            let Some(n) = operand_len(opcode) else {
                self.at = self.list.len();
                return None;
            };
            let Some(o) = self.list.get(self.at + 1..self.at + 1 + n) else {
                self.at = self.list.len();
                return None;
            };
            self.at += 1 + n;
            let rgba = self.rgba;
            let radius = || u16::from_le_bytes([o[0], o[1]]);
            let (x, y) = self.point;
            match opcode {
                op::COLOUR => self.rgba = [o[0], o[1], o[2], o[3]],
                op::WIDTH => self.width = o[0].max(1),
                op::ORIGIN => self.origin = (u16::from_le_bytes([o[0], o[1]]), u16::from_le_bytes([o[2], o[3]])),
                op::MOVE => self.point = self.point(o, 0),
                op::LINE => {
                    let (x1, y1) = self.point(o, 0);
                    self.point = (x1, y1);
                    return Some(Primitive::Line { x0: x, y0: y, x1, y1, width: self.width, rgba });
                }
                op::CIRCLE => return Some(Primitive::Circle { x, y, radius: radius(), width: self.width, rgba }),
                op::DISC => return Some(Primitive::Disc { x, y, radius: radius(), rgba }),
                op::RECT => {
                    let (x1, y1) = self.point(o, 0);
                    return Some(Primitive::Rect { x0: x, y0: y, x1, y1, rgba });
                }
                op::TRIANGLE => {
                    let ((x1, y1), (x2, y2)) = (self.point(o, 0), self.point(o, 1));
                    return Some(Primitive::Triangle { x0: x, y0: y, x1, y1, x2, y2, rgba });
                }
                _ => unreachable!(),
            }
        }
    }
}

/// The decoded list as text, one primitive per line (see `Display`), as
/// the clean style with fill on draws it.
pub fn dump(list: &[u8]) -> String {
    decode(list).map(|p| format!("{p}\n")).collect()
}

/// The list as outlines (SPEC §vector.list: the phosphor style, and fill off): nothing is filled. A
/// disc becomes a circle, a rectangle four lines and a triangle three lines,
/// each with the width in effect at that command; lines and circles pass
/// through. Every host draws phosphor shapes from this, so they agree on
/// which outlines a fill becomes.
pub fn decode_outlines(list: &[u8]) -> Outlines<'_> {
    decode_outlines_within(list, INK_CAP)
}

/// [`decode_outlines`] with the ink cap lowered to `cap` (at most
/// [`INK_CAP`]).
pub fn decode_outlines_within(list: &[u8], cap: u64) -> Outlines<'_> {
    let ink = Ink { off: false, total: 0, cap: cap.min(INK_CAP) };
    Outlines { decoder: decode_uncapped(list), queued: [None; 4], next: 4, ink, done: false }
}

/// The ink the shapes of `list` that are drawn within `cap` count (SPEC
/// §vector.ink-cap): as outlines when `outlined`.
pub fn drawn_ink(list: &[u8], outlined: bool, cap: u64) -> u64 {
    if outlined { decode_outlines_within(list, cap).map(|p| ink(&p)).sum() } else { decode_within(list, cap).map(|p| ink(&p)).sum() }
}

/// What `list` asks of a host before the ink cap (SPEC §vector.ink-cap),
/// in square screen units: the ink of its filled shapes (0 when
/// `outlined`, where nothing is filled) and that of its lines and circle
/// outlines (and, when `outlined`, of the outlines of its filled shapes).
pub fn work(list: &[u8], outlined: bool) -> (u64, u64) {
    let decoder = decode_uncapped(list);
    if outlined {
        let all = Outlines { decoder, queued: [None; 4], next: 4, ink: Ink::OFF, done: false };
        return (0, all.map(|p| stroke_ink(&p)).sum());
    }
    decoder.fold((0, 0), |(f, s), p| (f + fill_ink(&p), s + stroke_ink(&p)))
}

/// Iterator over the outline primitives of a list; see [`decode_outlines`].
/// It yields only [`Primitive::Line`] and [`Primitive::Circle`].
#[derive(Clone, Debug)]
pub struct Outlines<'a> {
    decoder: Decoder<'a>,
    /// Lines of the last rectangle or triangle not yet returned, from `next`.
    queued: [Option<Primitive>; 4],
    next: usize,
    /// The ink cap, over the outlines given.
    ink: Ink,
    /// The cap stopped an outline: nothing more is given.
    done: bool,
}

impl Iterator for Outlines<'_> {
    type Item = Primitive;

    fn next(&mut self) -> Option<Primitive> {
        if self.done {
            return None;
        }
        let p = self.next_outline()?;
        if self.ink.admit(&p) {
            return Some(p);
        }
        self.done = true;
        None
    }
}

impl Outlines<'_> {
    fn next_outline(&mut self) -> Option<Primitive> {
        while self.next < self.queued.len() {
            self.next += 1;
            if let Some(p) = self.queued[self.next - 1] {
                return Some(p);
            }
        }
        let p = self.decoder.next()?;
        // The width the decoder holds now is the one in effect at `p`.
        let width = self.decoder.width;
        let (corners, n, rgba) = match p {
            Primitive::Line { .. } | Primitive::Circle { .. } | Primitive::Beam { .. } => return Some(p),
            Primitive::Disc { x, y, radius, rgba } => return Some(Primitive::Circle { x, y, radius, width, rgba }),
            Primitive::Rect { x0, y0, x1, y1, rgba } => ([(x0, y0), (x1, y0), (x1, y1), (x0, y1)], 4, rgba),
            Primitive::Triangle { x0, y0, x1, y1, x2, y2, rgba } => ([(x0, y0), (x1, y1), (x2, y2), (0, 0)], 3, rgba),
        };
        self.queued = [None; 4];
        for i in 0..n {
            let ((x0, y0), (x1, y1)) = (corners[i], corners[(i + 1) % n]);
            self.queued[i] = Some(Primitive::Line { x0, y0, x1, y1, width, rgba });
        }
        self.next = 1;
        self.queued[0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Rgba = [255; 4];

    fn xy(x: i16, y: i16) -> [u8; 4] {
        let (a, b) = (x.to_le_bytes(), y.to_le_bytes());
        [a[0], a[1], b[0], b[1]]
    }

    fn cmd(opcode: u8, operands: &[u8]) -> Vec<u8> {
        [&[opcode][..], operands].concat()
    }

    fn all(list: &[u8]) -> Vec<Primitive> {
        decode(list).collect()
    }

    #[test]
    fn every_opcode() {
        let list = [
            cmd(op::MOVE, &xy(10, 20)),
            cmd(op::LINE, &xy(30, -40)),
            cmd(op::COLOUR, &[1, 2, 3, 4]),
            cmd(op::WIDTH, &[0]),
            cmd(op::CIRCLE, &500u16.to_le_bytes()),
            cmd(op::WIDTH, &[48]),
            cmd(op::LINE, &xy(30, -40)),
            cmd(op::DISC, &0xFFFFu16.to_le_bytes()),
            cmd(op::RECT, &xy(-5, 7)),
            [cmd(op::TRIANGLE, &xy(1, 2)), xy(3, 4).to_vec()].concat(),
            cmd(op::ORIGIN, &xy(100, 200)),
            cmd(op::MOVE, &xy(1, 1)),
            cmd(op::ORIGIN, &xy(-1, -1)), // replaces, does not add to, the origin
            cmd(op::LINE, &xy(1, 1)),
            cmd(op::END, &[]),
            cmd(op::LINE, &xy(9, 9)),
        ]
        .concat();
        let c = [1, 2, 3, 4];
        assert_eq!(
            all(&list),
            [
                Primitive::Line { x0: 10, y0: 20, x1: 30, y1: -40, width: 16, rgba: WHITE },
                Primitive::Circle { x: 30, y: -40, radius: 500, width: 1, rgba: c },
                Primitive::Line { x0: 30, y0: -40, x1: 30, y1: -40, width: 48, rgba: c },
                Primitive::Disc { x: 30, y: -40, radius: 0xFFFF, rgba: c },
                Primitive::Rect { x0: 30, y0: -40, x1: -5, y1: 7, rgba: c },
                Primitive::Triangle { x0: 30, y0: -40, x1: 1, y1: 2, x2: 3, y2: 4, rgba: c },
                Primitive::Line { x0: 101, y0: 201, x1: 0, y1: 0, width: 48, rgba: c },
            ]
        );
        assert_eq!(
            dump(&list).lines().take(2).collect::<Vec<_>>(),
            ["line 10 20 30 -40 w 16 c ffffffff", "circle 30 -40 r 500 w 1 c 01020304"]
        );
    }

    #[test]
    fn origin_wraps_to_16_bits() {
        let list = [cmd(op::ORIGIN, &xy(0x7FF0, -2)), cmd(op::MOVE, &xy(0x20, 1)), cmd(op::RECT, &xy(-0x7FF0, -0x7FFF))]
            .concat();
        assert_eq!(all(&list), [Primitive::Rect { x0: -0x7FF0, y0: -1, x1: 0, y1: 0x7FFF, rgba: WHITE }]);
    }

    #[test]
    fn termination_rules() {
        let line = cmd(op::LINE, &xy(5, 5));
        let one = [Primitive::Line { x0: 0, y0: 0, x1: 5, y1: 5, width: 16, rgba: WHITE }];
        // End of the submitted bytes.
        assert_eq!(all(&line), one);
        // End command.
        assert_eq!(all(&[line.clone(), vec![op::END], line.clone()].concat()), one);
        // Unknown opcode.
        for unknown in [0x0A, 0x80, 0xFF] {
            assert_eq!(all(&[line.clone(), vec![unknown], line.clone()].concat()), one);
        }
        // Operands cut off: every proper prefix of each command's operands.
        for c in [cmd(op::LINE, &xy(9, 9)), cmd(op::DISC, &[9, 0]), cmd(op::COLOUR, &[1, 2, 3, 4]), cmd(op::WIDTH, &[3])] {
            for cut in 1..c.len() {
                assert_eq!(all(&[line.clone(), c[..cut].to_vec()].concat()), one, "{c:?} cut at {cut}");
            }
        }
        assert_eq!(all(&[]), []);
        assert_eq!(all(&[op::END]), []);
    }

    #[test]
    fn reset_state() {
        let v = Vector::default();
        assert!(v.list().is_empty());
        assert_eq!((v.style(), v.persistence(), v.fill()), (STYLE_CLEAN, 160, true));
    }

    #[test]
    fn submit_copies_and_counts_changes() {
        let list = [cmd(op::MOVE, &xy(1, 2)), cmd(op::LINE, &xy(3, 4))].concat();
        let mut v = Vector::default();
        assert_eq!(v.submit(&list), RESULT_DONE);
        assert_eq!(v.list(), list);
        let g = v.generation();
        // Same bytes again: list unchanged.
        assert_eq!(v.submit(&list), RESULT_DONE);
        assert_eq!(v.generation(), g);
        assert_eq!(v.submit(&list[..5]), RESULT_DONE);
        assert_eq!(v.list(), &list[..5]);
        assert_ne!(v.generation(), g);
        assert_eq!(v.submit(&vec![0; MAX_LIST]), RESULT_DONE);
    }

    #[test]
    fn rejections_keep_the_list() {
        let list = cmd(op::DISC, &[16, 0]);
        let mut v = Vector::default();
        v.submit(&list);
        let g = v.generation();
        for bad in [&[][..], &vec![0; MAX_LIST + 1]] {
            assert_eq!(v.submit(bad), RESULT_REJECTED);
            assert_eq!(v.list(), list);
            assert_eq!(v.generation(), g);
        }
    }

    #[test]
    fn clear() {
        let mut v = Vector::default();
        v.clear();
        v.submit(&cmd(op::DISC, &[16, 0]));
        let g = v.generation();
        v.clear();
        assert!(v.list().is_empty());
        assert_ne!(v.generation(), g);
        assert_eq!(v.shapes().count(), 0);
    }

    #[test]
    fn style_persistence_and_fill() {
        let mut v = Vector::default();
        assert_eq!(v.style_line(), None);
        assert_eq!(v.set_style(1), RESULT_DONE);
        assert!(v.phosphor() && v.outlined());
        assert_eq!(v.style_line().unwrap(), "-- vector style: phosphor, persistence 160 --");
        for bad in [2, 0x100, u32::MAX] {
            assert_eq!(v.set_style(bad), RESULT_REJECTED);
            assert_eq!(v.style(), STYLE_PHOSPHOR);
        }
        assert_eq!(v.set_style(0), RESULT_DONE);
        for p in [0, 1, 255] {
            assert_eq!(v.set_persistence(p), RESULT_DONE);
            assert_eq!(v.persistence(), p as u8);
        }
        assert_eq!(v.set_persistence(256), RESULT_REJECTED);
        assert_eq!(v.persistence(), 255);
        assert_eq!(v.set_fill(1), RESULT_DONE);
        assert!(!v.fill() && v.outlined() && !v.phosphor());
        assert_eq!(v.style_line().unwrap(), "-- vector style: clean, fill off --");
        assert_eq!(v.set_fill(2), RESULT_REJECTED);
        assert!(!v.fill());
        let mut state = Vec::new();
        v.write_state(&mut state);
        assert_eq!(&state[..7], &[STYLE_CLEAN, 255, 1, 0, 0, 0, 0]);
    }

    #[test]
    fn fill_off_draws_outlines() {
        let list = [cmd(op::MOVE, &xy(10, 20)), cmd(op::RECT, &xy(30, 40)), cmd(op::DISC, &[5, 0])].concat();
        let mut v = Vector::default();
        v.submit(&list);
        assert_eq!(v.shapes().collect::<Vec<_>>(), all(&list));
        v.set_fill(1);
        assert_eq!(v.shapes().collect::<Vec<_>>(), decode_outlines(&list).collect::<Vec<_>>());
        assert_eq!(v.shapes().count(), 5);
    }

    #[test]
    fn fill_ink_is_half_the_box_clipped_to_the_screen() {
        let c = WHITE;
        let disc = |x, y, radius| Primitive::Disc { x, y, radius, rgba: c };
        assert_eq!(fill_ink(&disc(160, 160, 16)), 2); // 2 x 2 units, halved
        assert_eq!(fill_ink(&disc(0, 0, 16)), 1); // a quarter on screen, rounded up
        assert_eq!(fill_ink(&disc(5120, 3840, 60000)), 640 * 480 / 2);
        assert_eq!(fill_ink(&disc(-100, 0, 50)), 0);
        assert_eq!(fill_ink(&disc(0, 0, 1)), 1, "rounded up");
        assert_eq!(fill_ink(&disc(16, 16, 0)), 0);
        let rect = Primitive::Rect { x0: 320, y0: 0, x1: -32000, y1: 32000, rgba: c };
        assert_eq!(fill_ink(&rect), 10 * 480);
        let tri = Primitive::Triangle { x0: 0, y0: 0, x1: 160, y1: 0, x2: 80, y2: 320, rgba: c };
        assert_eq!(fill_ink(&tri), 100);
        assert_eq!(fill_ink(&Primitive::Line { x0: 0, y0: 0, x1: 9000, y1: 7000, width: 255, rgba: c }), 0);
    }

    #[test]
    fn stroke_ink_is_the_stroke_or_its_box() {
        let c = WHITE;
        let line = |x0, y0, x1, y1, width| Primitive::Line { x0, y0, x1, y1, width, rgba: c };
        // 100 units long, 1 wide: (1600 + 16) x (16 + 48) / 256.
        assert_eq!(stroke_ink(&line(1000, 1000, 2600, 1000, 16)), 404);
        // A dot of 1/16: (0 + 1) x 49, rounded up.
        assert_eq!(stroke_ink(&line(1000, 1000, 1000, 1000, 1)), 1);
        // Diagonal: |dx| + |dy|.
        assert_eq!(stroke_ink(&line(0, 0, 1600, 1600, 16)), (3216 * 64u64).div_ceil(256));
        // Off the screen: its box is empty.
        assert_eq!(stroke_ink(&line(-9000, -9000, -5000, -9000, 255)), 0);
        // Along the screen's edge, half outside: the box clipped (151
        // sixteenths high), smaller than the stroke.
        let edge = line(0, 0, 10240, 0, 255);
        assert_eq!(stroke_ink(&edge), (10240u64 * 151).div_ceil(256));
        let circle = Primitive::Circle { x: 5120, y: 3840, radius: 1600, width: 32, rgba: c };
        assert_eq!(stroke_ink(&circle), ((8 * 1600 + 32) * 80u64).div_ceil(256));
        assert_eq!(stroke_ink(&Primitive::Rect { x0: 0, y0: 0, x1: 99, y1: 99, rgba: c }), 0);
    }

    #[test]
    fn ink_cap_stops_the_rest_of_the_list() {
        // A full-screen rectangle (153,600) and a half-screen one (76,800)
        // make the cap exactly; the next shape, however small, is not
        // drawn, nor anything after it.
        let full = cmd(op::RECT, &xy(10240, 7680));
        let half = cmd(op::RECT, &xy(10240, 3840));
        let mut list = [full.clone(), half.clone()].concat();
        assert_eq!(all(&list).iter().map(ink).sum::<u64>(), INK_CAP);
        list.extend_from_slice(&cmd(op::LINE, &xy(5, 5)));
        list.extend_from_slice(&cmd(op::CIRCLE, &[9, 0]));
        assert_eq!(all(&list).len(), 2);
        // A shape that would cross the cap is dropped, and so is a smaller
        // one after it that would fit.
        let list = [full.clone(), full.clone(), cmd(op::LINE, &xy(5, 5))].concat();
        assert_eq!(all(&list).len(), 1);
        // Outlines count too: full-screen rectangles' lines of the
        // greatest width, up to the cap.
        let wide = [cmd(op::WIDTH, &[255]), full.clone()].concat();
        let many: Vec<u8> = [wide, std::iter::repeat_n(full.clone(), 30).flatten().collect()].concat();
        let drawn: Vec<Primitive> = decode_outlines(&many).collect();
        let total: u64 = drawn.iter().map(stroke_ink).sum();
        assert!(total <= INK_CAP && total + 4000 > INK_CAP, "{total}");
        assert_eq!(work(&many, true).1, decode_outlines_uncapped(&many).map(|p| stroke_ink(&p)).sum::<u64>());
        assert!(work(&many, true).1 > INK_CAP);
        assert_eq!(work(&many, false), (31 * 153_600, 0));
    }

    /// The outlines without the cap.
    fn decode_outlines_uncapped(list: &[u8]) -> Outlines<'_> {
        Outlines { decoder: decode_uncapped(list), queued: [None; 4], next: 4, ink: Ink::OFF, done: false }
    }

    #[test]
    fn outlines() {
        let c = [1, 2, 3, 4];
        let list = [
            cmd(op::COLOUR, &c),
            cmd(op::WIDTH, &[24]),
            cmd(op::MOVE, &xy(10, 20)),
            cmd(op::DISC, &500u16.to_le_bytes()),
            cmd(op::RECT, &xy(30, 40)),
            cmd(op::WIDTH, &[0]),
            [cmd(op::TRIANGLE, &xy(50, 20)), xy(10, 60).to_vec()].concat(),
            cmd(op::CIRCLE, &7u16.to_le_bytes()),
            cmd(op::LINE, &xy(-1, -2)),
        ]
        .concat();
        let line = |x0, y0, x1, y1, width| Primitive::Line { x0, y0, x1, y1, width, rgba: c };
        let want = [
            Primitive::Circle { x: 10, y: 20, radius: 500, width: 24, rgba: c },
            line(10, 20, 30, 20, 24),
            line(30, 20, 30, 40, 24),
            line(30, 40, 10, 40, 24),
            line(10, 40, 10, 20, 24),
            line(10, 20, 50, 20, 1),
            line(50, 20, 10, 60, 1),
            line(10, 60, 10, 20, 1),
            Primitive::Circle { x: 10, y: 20, radius: 7, width: 1, rgba: c },
            line(10, 20, -1, -2, 1),
        ];
        assert_eq!(decode_outlines(&list).collect::<Vec<_>>(), want);
        // Lists without fills are unchanged.
        let plain = [cmd(op::LINE, &xy(5, 5)), cmd(op::CIRCLE, &[9, 0])].concat();
        assert_eq!(decode_outlines(&plain).collect::<Vec<_>>(), all(&plain));
        assert_eq!(decode_outlines(&[]).count(), 0);
    }
}
