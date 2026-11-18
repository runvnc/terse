//! Device 3, 2D vector display (SPEC 8).
//!
//! The device only validates and copies the display list. [`decode`] turns a
//! stored list into resolved primitives (origin applied, drawing state
//! tracked, SPEC 8.3 termination rules); hosts and tests draw from it rather
//! than parsing the list themselves.

use std::fmt;

use crate::memory::Memory;

/// Longest list a submit accepts (SPEC 8.2).
pub const MAX_LIST: usize = 16_384;

/// Commands (port 35h writes).
pub const CMD_SUBMIT: u8 = 0x01;
pub const CMD_CLEAR: u8 = 0x02;
/// Results (port 35h reads).
pub const RESULT_DONE: u8 = 0x00;
pub const RESULT_REJECTED: u8 = 0x01;

/// Display-list opcodes (SPEC 8.3).
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

/// The device state (SPEC 8.5): address, length, result and the list copy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Vector {
    /// Linear RAM address, 24 bits.
    addr: u32,
    length: u16,
    result: u8,
    list: Vec<u8>,
    /// Changes whenever the list's contents change (not part of the
    /// machine state; for hosts that cache what they upload).
    generation: u64,
}

impl Vector {
    /// Returns the charge in T-states (SPEC 8.4).
    pub fn port_out(&mut self, reg: u8, value: u8, memory: &Memory) -> u32 {
        match reg {
            0x0 => self.addr = (self.addr & 0xFF_FF00) | value as u32,
            0x1 => self.addr = (self.addr & 0xFF_00FF) | (value as u32) << 8,
            0x2 => self.addr = (self.addr & 0x00_FFFF) | (value as u32) << 16,
            0x3 => self.length = (self.length & 0xFF00) | value as u16,
            0x4 => self.length = (self.length & 0x00FF) | (value as u16) << 8,
            0x5 => return self.command(value, memory),
            _ => {}
        }
        0
    }

    pub fn port_in(&self, reg: u8) -> u8 {
        match reg {
            0x5 => self.result,
            _ => 0xFF,
        }
    }

    fn command(&mut self, value: u8, memory: &Memory) -> u32 {
        match value {
            CMD_SUBMIT => {
                let (addr, len, ram) = (self.addr as usize, self.length as usize, memory.ram());
                if len == 0 || len > MAX_LIST || addr + len > ram.len() {
                    self.result = RESULT_REJECTED;
                    return 0;
                }
                self.result = RESULT_DONE;
                let bytes = &ram[addr..addr + len];
                if self.list != bytes {
                    self.list.clear();
                    self.list.extend_from_slice(bytes);
                    self.generation += 1;
                }
                len as u32
            }
            CMD_CLEAR => {
                self.result = RESULT_DONE;
                if !self.list.is_empty() {
                    self.list.clear();
                    self.generation += 1;
                }
                0
            }
            _ => {
                self.result = RESULT_REJECTED;
                0
            }
        }
    }

    /// The list the device holds (the bytes of the last successful submit).
    pub fn list(&self) -> &[u8] {
        &self.list
    }

    /// A counter that changes whenever [`Vector::list`] changes.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The list decoded into primitives.
    pub fn primitives(&self) -> Decoder<'_> {
        decode(&self.list)
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
        }
    }
}

/// Decodes a display list (SPEC 8.3).
pub fn decode(list: &[u8]) -> Decoder<'_> {
    Decoder { list, at: 0, rgba: [255; 4], width: 16, origin: (0, 0), point: (0, 0) }
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

/// The decoded list as text, one primitive per line (see `Display`).
pub fn dump(list: &[u8]) -> String {
    decode(list).map(|p| format!("{p}\n")).collect()
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

    fn ram_with(bytes: &[u8], at: usize) -> Memory {
        let mut m = Memory::new(&[0], 4);
        m.ram_mut()[at..at + bytes.len()].copy_from_slice(bytes);
        m
    }

    /// Sets address and length through the ports, then submits.
    fn submit(v: &mut Vector, m: &Memory, addr: u32, len: u16) -> u32 {
        for (reg, b) in [(0, addr as u8), (1, (addr >> 8) as u8), (2, (addr >> 16) as u8), (3, len as u8), (4, (len >> 8) as u8)] {
            assert_eq!(v.port_out(reg, b, m), 0);
        }
        v.port_out(5, CMD_SUBMIT, m)
    }

    #[test]
    fn reset_state() {
        let v = Vector::default();
        assert_eq!(v.port_in(5), RESULT_DONE);
        assert!(v.list().is_empty());
        for reg in [0, 1, 2, 3, 4, 6, 0xF] {
            assert_eq!(v.port_in(reg), 0xFF, "port 3{reg:X}h is write-only or undefined");
        }
    }

    #[test]
    fn submit_copies_and_charges() {
        let list = [cmd(op::MOVE, &xy(1, 2)), cmd(op::LINE, &xy(3, 4))].concat();
        let mut m = ram_with(&list, 0x6123);
        let mut v = Vector::default();
        assert_eq!(submit(&mut v, &m, 0x6123, list.len() as u16), list.len() as u32, "1 T-state per byte");
        assert_eq!(v.port_in(5), RESULT_DONE);
        assert_eq!(v.list(), list);
        let g = v.generation();
        // The device keeps its own copy.
        m.ram_mut()[0x6123] = 0xFF;
        assert_eq!(v.list(), list);
        // Same bytes again: still charged, list unchanged.
        m.ram_mut()[0x6123] = op::MOVE;
        assert_eq!(submit(&mut v, &m, 0x6123, list.len() as u16), list.len() as u32);
        assert_eq!(v.generation(), g);
        // Different bytes: the counter changes.
        assert_eq!(submit(&mut v, &m, 0x6123, 5), 5);
        assert_eq!(v.list(), &list[..5]);
        assert_ne!(v.generation(), g);
        // Largest list, ending exactly at the end of RAM.
        let end = m.ram().len() as u32;
        assert_eq!(submit(&mut v, &m, end - 16_384, 16_384), 16_384);
        assert_eq!(v.list().len(), 16_384);
    }

    #[test]
    fn rejections_keep_the_list() {
        let list = cmd(op::DISC, &[16, 0]);
        let m = ram_with(&list, 0);
        let mut v = Vector::default();
        submit(&mut v, &m, 0, 3);
        let g = v.generation();
        let end = m.ram().len() as u32;
        for (addr, len) in [(0, 0), (0, 16_385), (end - 2, 3), (end, 1), (0xFF_FFFF, 1), (end - 16_383, 16_384)] {
            assert_eq!(submit(&mut v, &m, addr, len), 0, "no charge for {addr:X} {len}");
            assert_eq!(v.port_in(5), RESULT_REJECTED, "{addr:X} {len}");
            assert_eq!(v.list(), list);
            assert_eq!(v.generation(), g);
        }
        // Unknown commands.
        for c in [0x00, 0x03, 0xFF] {
            assert_eq!(v.port_out(5, c, &m), 0);
            assert_eq!(v.port_in(5), RESULT_REJECTED);
            assert_eq!(v.list(), list);
        }
        // A good submit sets the result back to done.
        submit(&mut v, &m, 0, 3);
        assert_eq!(v.port_in(5), RESULT_DONE);
    }

    #[test]
    fn clear() {
        let m = ram_with(&cmd(op::DISC, &[16, 0]), 0);
        let mut v = Vector::default();
        v.port_out(5, 0x7F, &m);
        assert_eq!(v.port_out(5, CMD_CLEAR, &m), 0);
        assert_eq!(v.port_in(5), RESULT_DONE);
        submit(&mut v, &m, 0, 3);
        let g = v.generation();
        assert_eq!(v.port_out(5, CMD_CLEAR, &m), 0, "clear is free");
        assert!(v.list().is_empty());
        assert_ne!(v.generation(), g);
        assert_eq!(v.primitives().count(), 0);
    }
}
