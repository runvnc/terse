//! Device 2, Terminal (SPEC 6).

use crate::wide::is_wide;

pub const COLS: usize = 80;
pub const ROWS: usize = 30;
pub const CELL_BYTES: usize = 4;
pub const CELL_MEMORY: usize = COLS * ROWS * CELL_BYTES; // 9,600
/// Charge for clear, scroll up and scroll down (SPEC 6.5).
pub const SCROLL_CLEAR_CHARGE: u32 = 600;

/// The right cell of a wide character (SPEC 6.1).
pub const CONTINUATION: u16 = 0xFFFF;
const REPLACEMENT: u16 = 0xFFFD;
const SPACE: u16 = 0x0020;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: u16,
    pub fg: u8,
    pub bg: u8,
}

/// Incremental UTF-8 decoder for port 20h (SPEC 6.3).
///
/// Malformed input becomes U+FFFD following the "maximal subpart" practice
/// (Unicode ch. 3, WHATWG): an invalid lead byte gives one U+FFFD; a sequence
/// cut short by an unexpected byte gives one U+FFFD and that byte is decoded
/// afresh.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Utf8Decoder {
    code_point: u32,
    needed: u8,
    lower: u8,
    upper: u8,
    /// The bytes of the incomplete sequence so far (`len` of them): the
    /// decoder's state in a form any implementation can produce (STATE.md).
    bytes: [u8; 3],
    len: u8,
}

impl Utf8Decoder {
    /// Feeds one byte; returns up to two completed code points.
    fn feed(&mut self, byte: u8) -> [Option<u32>; 2] {
        if self.needed == 0 {
            return [self.start(byte), None];
        }
        if !(self.lower..=self.upper).contains(&byte) {
            *self = Utf8Decoder::default();
            return [Some(REPLACEMENT as u32), self.start(byte)];
        }
        self.lower = 0x80;
        self.upper = 0xBF;
        self.code_point = (self.code_point << 6) | (byte & 0x3F) as u32;
        self.needed -= 1;
        if self.needed == 0 {
            let cp = self.code_point;
            *self = Utf8Decoder::default();
            return [Some(cp), None];
        }
        self.bytes[self.len as usize] = byte;
        self.len += 1;
        [None, None]
    }

    fn start(&mut self, byte: u8) -> Option<u32> {
        let (needed, bits, lower, upper) = match byte {
            0x00..=0x7F => return Some(byte as u32),
            0xC2..=0xDF => (1, byte & 0x1F, 0x80, 0xBF),
            0xE0 => (2, byte & 0x0F, 0xA0, 0xBF),
            0xED => (2, byte & 0x0F, 0x80, 0x9F), // no surrogates
            0xE1..=0xEF => (2, byte & 0x0F, 0x80, 0xBF),
            0xF0 => (3, byte & 0x07, 0x90, 0xBF),
            0xF4 => (3, byte & 0x07, 0x80, 0x8F),
            0xF1..=0xF3 => (3, byte & 0x07, 0x80, 0xBF),
            _ => return Some(REPLACEMENT as u32),
        };
        *self = Utf8Decoder { code_point: bits as u32, needed, lower, upper, bytes: [byte, 0, 0], len: 1 };
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Terminal {
    cells: Vec<u8>,
    col: u8,
    row: u8,
    fg: u8,
    bg: u8,
    cursor_visible: bool,
    /// SPEC 6.3: the next printed character starts a new row first.
    pending_wrap: bool,
    /// Kept as written; reduced modulo 9,600 when used (SPEC 6.2).
    cell_pointer: u16,
    palette: [[u8; 3]; 256],
    palette_index: u8,
    /// Which component (0 red, 1 green, 2 blue) the next port 2Ah write sets.
    palette_component: u8,
    decoder: Utf8Decoder,
}

impl Default for Terminal {
    fn default() -> Self {
        // SPEC 6.6
        let mut t = Terminal {
            cells: vec![0; CELL_MEMORY],
            col: 0,
            row: 0,
            fg: 7,
            bg: 0,
            cursor_visible: true,
            pending_wrap: false,
            cell_pointer: 0,
            palette: xterm_palette(),
            palette_index: 0,
            palette_component: 0,
            decoder: Utf8Decoder::default(),
        };
        t.fill_rows(0..ROWS);
        t
    }
}

impl Terminal {
    pub fn cell(&self, col: usize, row: usize) -> Cell {
        let i = (row * COLS + col) * CELL_BYTES;
        let c = &self.cells[i..i + CELL_BYTES];
        Cell { ch: u16::from_le_bytes([c[0], c[1]]), fg: c[2], bg: c[3] }
    }

    /// Raw cell memory, 4 bytes per cell (SPEC 6.1).
    pub fn cell_bytes(&self) -> &[u8] {
        &self.cells
    }

    pub fn palette(&self) -> &[[u8; 3]; 256] {
        &self.palette
    }

    pub fn cursor(&self) -> (u8, u8) {
        (self.col, self.row)
    }

    pub fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    pub fn pending_wrap(&self) -> bool {
        self.pending_wrap
    }

    pub fn colors(&self) -> (u8, u8) {
        (self.fg, self.bg)
    }

    pub fn cell_pointer(&self) -> u16 {
        self.cell_pointer
    }

    /// The grid as text, one line per row, trailing spaces trimmed. The
    /// continuation cell of a wide pair is skipped (the wide character already
    /// takes two columns); a continuation code anywhere else shows as a space.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for row in 0..ROWS {
            let mut line = String::new();
            for col in 0..COLS {
                let ch = self.cell(col, row).ch;
                if ch == CONTINUATION {
                    if !(col > 0 && self.is_pair(col - 1, row)) {
                        line.push(' ');
                    }
                } else {
                    line.push(char::from_u32(ch as u32).unwrap_or('\u{FFFD}'));
                }
            }
            out.push_str(line.trim_end_matches(' '));
            out.push('\n');
        }
        out
    }

    /// Whether cells `col` and `col + 1` of `row` form a wide pair: a
    /// character followed by the continuation code.
    fn is_pair(&self, col: usize, row: usize) -> bool {
        col + 1 < COLS && self.cell(col, row).ch != CONTINUATION && self.cell(col + 1, row).ch == CONTINUATION
    }

    /// Cells, cursor, colours, cell pointer, palette and the put-character
    /// decoder (STATE.md, terminal section).
    pub(crate) fn write_state(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.cells);
        out.extend_from_slice(&[self.col, self.row, self.fg, self.bg]);
        out.extend_from_slice(&[self.cursor_visible as u8, self.pending_wrap as u8]);
        out.extend_from_slice(&self.cell_pointer.to_le_bytes());
        out.extend(self.palette.iter().flatten());
        out.extend_from_slice(&[self.palette_index, self.palette_component]);
        out.push(self.decoder.len);
        out.extend_from_slice(&self.decoder.bytes);
    }

    /// Returns the charge in T-states.
    pub fn port_out(&mut self, reg: u8, value: u8) -> u32 {
        match reg {
            0x0 => {
                let mut charge = 0;
                for cp in self.decoder.feed(value).into_iter().flatten() {
                    charge += self.put_char(cp);
                }
                charge
            }
            0x1 => {
                self.col = value.min(COLS as u8 - 1);
                self.pending_wrap = false;
                0
            }
            0x2 => {
                self.row = value.min(ROWS as u8 - 1);
                self.pending_wrap = false;
                0
            }
            0x3 => {
                self.fg = value;
                0
            }
            0x4 => {
                self.bg = value;
                0
            }
            0x5 => self.command(value),
            0x6 => {
                self.cell_pointer = (self.cell_pointer & 0xFF00) | value as u16;
                0
            }
            0x7 => {
                self.cell_pointer = (self.cell_pointer & 0x00FF) | (value as u16) << 8;
                0
            }
            0x8 => {
                let p = self.take_cell_pointer();
                self.cells[p] = value;
                0
            }
            0x9 => {
                self.palette_index = value;
                self.palette_component = 0;
                0
            }
            0xA => {
                self.palette[self.palette_index as usize][self.palette_component as usize] = value;
                self.palette_component += 1;
                if self.palette_component == 3 {
                    self.palette_component = 0;
                    self.palette_index = self.palette_index.wrapping_add(1);
                }
                0
            }
            _ => 0,
        }
    }

    pub fn port_in(&mut self, reg: u8) -> u8 {
        match reg {
            0x1 => self.col,
            0x2 => self.row,
            0x3 => self.fg,
            0x4 => self.bg,
            0x6 => self.cell_pointer as u8,
            0x7 => (self.cell_pointer >> 8) as u8,
            0x8 => {
                let p = self.take_cell_pointer();
                self.cells[p]
            }
            0x9 => self.palette_index,
            0xB => COLS as u8,
            0xC => ROWS as u8,
            _ => 0xFF,
        }
    }

    /// Port 28h: the reduced pointer to use now; the pointer then advances.
    fn take_cell_pointer(&mut self) -> usize {
        let p = self.cell_pointer as usize % CELL_MEMORY;
        self.cell_pointer = ((p + 1) % CELL_MEMORY) as u16;
        p
    }

    /// SPEC 6.3; returns the charge.
    fn put_char(&mut self, cp: u32) -> u32 {
        let charge = match cp {
            0x08 => {
                self.col = self.col.saturating_sub(1);
                0
            }
            0x09 => {
                self.col = ((self.col / 8 + 1) * 8).min(COLS as u8 - 1);
                0
            }
            0x0A => {
                self.col = 0;
                self.next_row()
            }
            0x0C => self.clear(),
            0x0D => {
                self.col = 0;
                0
            }
            // Ignored entirely, so pending wrap survives them.
            0x00..=0x1F | 0x7F => return 0,
            // U+FFFF would forge a continuation cell, so it is printed like a
            // code point above FFFFh.
            _ => return self.print(u16::try_from(cp).ok().filter(|&c| c != CONTINUATION).unwrap_or(REPLACEMENT)),
        };
        self.pending_wrap = false;
        charge
    }

    /// Prints one character (SPEC 6.3 steps 1-4); returns the charge.
    fn print(&mut self, ch: u16) -> u32 {
        const LAST: usize = COLS - 1;
        let width = if is_wide(ch) { 2 } else { 1 };
        let mut charge = 0;
        if self.pending_wrap || (width == 2 && self.col as usize == LAST) {
            if !self.pending_wrap {
                self.write_cells(LAST, &[SPACE]);
            }
            self.pending_wrap = false;
            self.col = 0;
            charge = self.next_row();
        }
        let col = self.col as usize;
        self.write_cells(col, &[ch, CONTINUATION][..width]);
        if col + width > LAST {
            self.col = LAST as u8;
            self.pending_wrap = true;
        } else {
            self.col += width as u8;
        }
        charge
    }

    /// Writes `chars` from `col` in the cursor row with the current colours.
    /// A wide pair cut in two loses its other half, which becomes a space and
    /// keeps its colours (SPEC 6.3 step 3).
    fn write_cells(&mut self, col: usize, chars: &[u16]) {
        let row = self.row as usize;
        let end = col + chars.len();
        let broken = [
            (col > 0 && self.is_pair(col - 1, row)).then(|| col - 1),
            self.is_pair(end - 1, row).then_some(end),
        ];
        for c in broken.into_iter().flatten() {
            let i = (row * COLS + c) * CELL_BYTES;
            self.cells[i..i + 2].copy_from_slice(&SPACE.to_le_bytes());
        }
        for (i, &ch) in chars.iter().enumerate() {
            self.set_cell(col + i, row, ch);
        }
    }

    fn next_row(&mut self) -> u32 {
        if self.row as usize == ROWS - 1 {
            self.scroll_up()
        } else {
            self.row += 1;
            0
        }
    }

    /// SPEC 6.4; returns the charge.
    fn command(&mut self, value: u8) -> u32 {
        match value {
            0x01 => self.clear(),
            0x02 => self.scroll_up(),
            0x03 => {
                let row = COLS * CELL_BYTES;
                self.cells.copy_within(0..CELL_MEMORY - row, row);
                self.fill_rows(0..1);
                SCROLL_CLEAR_CHARGE
            }
            0x04 => {
                for col in self.col as usize..COLS {
                    self.set_cell(col, self.row as usize, SPACE);
                }
                0
            }
            0x05 => {
                self.cursor_visible = true;
                0
            }
            0x06 => {
                self.cursor_visible = false;
                0
            }
            _ => 0,
        }
    }

    fn clear(&mut self) -> u32 {
        self.fill_rows(0..ROWS);
        self.col = 0;
        self.row = 0;
        self.pending_wrap = false;
        SCROLL_CLEAR_CHARGE
    }

    fn scroll_up(&mut self) -> u32 {
        let row = COLS * CELL_BYTES;
        self.cells.copy_within(row.., 0);
        self.fill_rows(ROWS - 1..ROWS);
        SCROLL_CLEAR_CHARGE
    }

    fn set_cell(&mut self, col: usize, row: usize, ch: u16) {
        let i = (row * COLS + col) * CELL_BYTES;
        let [lo, hi] = ch.to_le_bytes();
        self.cells[i..i + CELL_BYTES].copy_from_slice(&[lo, hi, self.fg, self.bg]);
    }

    fn fill_rows(&mut self, rows: std::ops::Range<usize>) {
        for row in rows {
            for col in 0..COLS {
                self.set_cell(col, row, SPACE);
            }
        }
    }
}

/// The xterm 256-colour palette (SPEC 6.6).
pub fn xterm_palette() -> [[u8; 3]; 256] {
    const BASE: [[u8; 3]; 16] = [
        [0x00, 0x00, 0x00],
        [0xCD, 0x00, 0x00],
        [0x00, 0xCD, 0x00],
        [0xCD, 0xCD, 0x00],
        [0x00, 0x00, 0xEE],
        [0xCD, 0x00, 0xCD],
        [0x00, 0xCD, 0xCD],
        [0xE5, 0xE5, 0xE5],
        [0x7F, 0x7F, 0x7F],
        [0xFF, 0x00, 0x00],
        [0x00, 0xFF, 0x00],
        [0xFF, 0xFF, 0x00],
        [0x5C, 0x5C, 0xFF],
        [0xFF, 0x00, 0xFF],
        [0x00, 0xFF, 0xFF],
        [0xFF, 0xFF, 0xFF],
    ];
    const CUBE: [u8; 6] = [0x00, 0x5F, 0x87, 0xAF, 0xD7, 0xFF];
    let mut p = [[0u8; 3]; 256];
    p[..16].copy_from_slice(&BASE);
    for i in 0..216 {
        p[16 + i] = [CUBE[i / 36], CUBE[i / 6 % 6], CUBE[i % 6]];
    }
    for i in 0..24 {
        let v = 8 + 10 * i as u8;
        p[232 + i] = [v, v, v];
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(t: &mut Terminal, s: &[u8]) -> u32 {
        s.iter().map(|&b| t.port_out(0, b)).sum()
    }

    fn ch(t: &Terminal, col: usize, row: usize) -> u16 {
        t.cell(col, row).ch
    }

    #[test]
    fn reset_state() {
        let t = Terminal::default();
        assert_eq!(t.cell(0, 0), Cell { ch: 0x20, fg: 7, bg: 0 });
        assert_eq!(t.cell(79, 29), Cell { ch: 0x20, fg: 7, bg: 0 });
        assert_eq!(t.cursor(), (0, 0));
        assert!(t.cursor_visible());
        assert_eq!(t.cell_pointer(), 0);
        assert_eq!(t.palette()[1], [0xCD, 0, 0]);
        assert_eq!(t.palette()[16], [0, 0, 0]);
        assert_eq!(t.palette()[196], [0xFF, 0, 0]);
        assert_eq!(t.palette()[231], [0xFF, 0xFF, 0xFF]);
        assert_eq!(t.palette()[232], [8, 8, 8]);
        assert_eq!(t.palette()[255], [0xEE, 0xEE, 0xEE]);
    }

    #[test]
    fn writes_with_current_colours_and_wraps() {
        let mut t = Terminal::default();
        t.port_out(3, 2);
        t.port_out(4, 4);
        t.port_out(1, 78);
        assert_eq!(put(&mut t, b"ABC"), 0);
        assert_eq!(t.cell(78, 0), Cell { ch: 'A' as u16, fg: 2, bg: 4 });
        assert_eq!(ch(&t, 79, 0), 'B' as u16);
        assert_eq!(ch(&t, 0, 1), 'C' as u16);
        assert_eq!(t.cursor(), (1, 1));
    }

    #[test]
    fn last_cell_defers_scroll_until_next_character() {
        let mut t = Terminal::default();
        put(&mut t, b"top");
        t.port_out(1, 79);
        t.port_out(2, 29);
        assert_eq!(put(&mut t, b"Z"), 0);
        assert_eq!(ch(&t, 79, 29), 'Z' as u16);
        assert_eq!(ch(&t, 0, 0), 't' as u16, "no scroll yet");
        assert_eq!(t.cursor(), (79, 29));
        assert!(t.pending_wrap());
        assert_eq!(put(&mut t, b"Y"), SCROLL_CLEAR_CHARGE);
        assert_eq!(ch(&t, 79, 28), 'Z' as u16);
        assert_eq!(ch(&t, 0, 29), 'Y' as u16);
        assert_eq!(ch(&t, 0, 0), ' ' as u16, "top row scrolled away");
        assert_eq!(t.cursor(), (1, 29));
        assert!(!t.pending_wrap());
    }

    #[test]
    fn pending_wrap_cleared_by_controls_and_cursor_ports() {
        let at_end = |t: &mut Terminal| {
            t.port_out(1, 79);
            t.port_out(2, 3);
            put(t, b"E");
            assert!(t.pending_wrap());
        };
        let mut t = Terminal::default();
        at_end(&mut t);
        put(&mut t, b"\n");
        assert_eq!((t.cursor(), t.pending_wrap()), ((0, 4), false), "LF moves one row, not two");
        put(&mut t, b"a");
        assert_eq!(ch(&t, 0, 4), 'a' as u16);

        at_end(&mut t);
        put(&mut t, b"\rb");
        assert_eq!(ch(&t, 0, 3), 'b' as u16, "CR then print stays on the row");
        assert_eq!(t.cursor(), (1, 3));

        at_end(&mut t);
        put(&mut t, b"\x08");
        assert_eq!((t.cursor(), t.pending_wrap()), ((78, 3), false));

        at_end(&mut t);
        put(&mut t, b"\t");
        assert_eq!((t.cursor(), t.pending_wrap()), ((79, 3), false));
        put(&mut t, b"c");
        assert_eq!(ch(&t, 79, 3), 'c' as u16, "tab cleared the wrap");

        for (port, value) in [(1, 79), (2, 3)] {
            at_end(&mut t);
            t.port_out(port, value);
            assert!(!t.pending_wrap());
            put(&mut t, b"d");
            assert_eq!(ch(&t, 79, 3), 'd' as u16);
        }

        at_end(&mut t);
        t.port_out(5, 1);
        assert_eq!((t.cursor(), t.pending_wrap()), ((0, 0), false));

        // Ignored characters and other commands leave it set.
        at_end(&mut t);
        put(&mut t, b"\x07\x1B\x7F");
        t.port_out(5, 4);
        t.port_out(3, 9);
        assert!(t.pending_wrap());
        put(&mut t, b"f");
        assert_eq!(ch(&t, 0, 4), 'f' as u16);
    }

    #[test]
    fn wide_characters() {
        let mut t = Terminal::default();
        t.port_out(3, 3);
        t.port_out(4, 5);
        assert_eq!(put(&mut t, "你x".as_bytes()), 0);
        assert_eq!(t.cell(0, 0), Cell { ch: 0x4F60, fg: 3, bg: 5 });
        assert_eq!(t.cell(1, 0), Cell { ch: CONTINUATION, fg: 3, bg: 5 });
        assert_eq!(ch(&t, 2, 0), 'x' as u16);
        assert_eq!(t.cursor(), (3, 0));
    }

    #[test]
    fn wide_at_column_78_fills_the_row() {
        let mut t = Terminal::default();
        t.port_out(1, 78);
        put(&mut t, "好".as_bytes());
        assert_eq!((ch(&t, 78, 0), ch(&t, 79, 0)), (0x597D, CONTINUATION));
        assert_eq!((t.cursor(), t.pending_wrap()), ((79, 0), true));
        put(&mut t, b"n");
        assert_eq!(ch(&t, 0, 1), 'n' as u16);
        assert_eq!((ch(&t, 78, 0), ch(&t, 79, 0)), (0x597D, CONTINUATION), "pair intact");
    }

    #[test]
    fn wide_at_column_79_moves_to_next_row() {
        let mut t = Terminal::default();
        t.port_out(1, 79);
        t.port_out(3, 6);
        t.port_out(4, 2);
        put(&mut t, "好".as_bytes());
        assert_eq!(t.cell(79, 0), Cell { ch: 0x20, fg: 6, bg: 2 }, "column 79 blanked with current colours");
        assert_eq!((ch(&t, 0, 1), ch(&t, 1, 1)), (0x597D, CONTINUATION));
        assert_eq!((t.cursor(), t.pending_wrap()), ((2, 1), false));

        // On the last row the move scrolls.
        t.port_out(1, 79);
        t.port_out(2, 29);
        assert_eq!(put(&mut t, "好".as_bytes()), SCROLL_CLEAR_CHARGE);
        assert_eq!((ch(&t, 0, 29), ch(&t, 1, 29), ch(&t, 79, 28)), (0x597D, CONTINUATION, 0x20));
        assert_eq!(t.cursor(), (2, 29));

        // With pending wrap set, column 79 keeps the character just printed.
        t.port_out(1, 79);
        t.port_out(2, 5);
        put(&mut t, "z好".as_bytes());
        assert_eq!(ch(&t, 79, 5), 'z' as u16);
        assert_eq!((ch(&t, 0, 6), ch(&t, 1, 6)), (0x597D, CONTINUATION));
    }

    #[test]
    fn overwriting_half_of_a_wide_pair() {
        // Each case: pairs at 0-1 and 2-3 in colours 1/2, then the cursor
        // column and text written in colours 3/4.
        let cases: &[(u8, &str, [u16; 5])] = &[
            (0, "x", ['x' as u16, 0x20, 0x4F60, CONTINUATION, 0x20]),
            (1, "x", [0x20, 'x' as u16, 0x4F60, CONTINUATION, 0x20]),
            (1, "好", [0x20, 0x597D, CONTINUATION, 0x20, 0x20]),
            (2, "好", [0x4F60, CONTINUATION, 0x597D, CONTINUATION, 0x20]),
            (3, "好", [0x4F60, CONTINUATION, 0x20, 0x597D, CONTINUATION]),
        ];
        for &(col, text, want) in cases {
            let mut t = Terminal::default();
            t.port_out(3, 1);
            t.port_out(4, 2);
            put(&mut t, "你你".as_bytes());
            t.port_out(1, col);
            t.port_out(3, 3);
            t.port_out(4, 4);
            put(&mut t, text.as_bytes());
            let got: Vec<u16> = (0..5).map(|c| ch(&t, c, 0)).collect();
            assert_eq!(got, want, "col {col} {text}");
            // Cells blanked by step 3 keep their colours.
            for c in 0..4 {
                let cell = t.cell(c, 0);
                let rewritten = c >= col as usize && c < col as usize + if text == "x" { 1 } else { 2 };
                assert_eq!((cell.fg, cell.bg), if rewritten { (3, 4) } else { (1, 2) }, "col {col} {text} cell {c}");
            }
        }
    }

    #[test]
    fn continuation_code_is_not_printable() {
        let mut t = Terminal::default();
        put(&mut t, "\u{FFFF}".as_bytes());
        assert_eq!(ch(&t, 0, 0), 0xFFFD);
        assert_eq!(t.cursor(), (1, 0));
    }

    #[test]
    fn text_skips_continuation_cells() {
        let mut t = Terminal::default();
        put(&mut t, "a你b".as_bytes());
        // A stray continuation code written through the cell pointer.
        t.port_out(6, (COLS * CELL_BYTES) as u8);
        t.port_out(7, ((COLS * CELL_BYTES) >> 8) as u8);
        t.port_out(8, 0xFF);
        t.port_out(8, 0xFF);
        put(&mut t, b"\n\nend");
        let text = t.text();
        let lines: Vec<&str> = text.lines().take(3).collect();
        assert_eq!(lines, ["a你b", "", "end"]);
        assert_eq!(ch(&t, 0, 1), CONTINUATION);
    }

    #[test]
    fn control_codes() {
        let mut t = Terminal::default();
        t.port_out(1, 5);
        put(&mut t, b"\x08");
        assert_eq!(t.cursor(), (4, 0));
        t.port_out(1, 0);
        put(&mut t, b"\x08");
        assert_eq!(t.cursor(), (0, 0));
        put(&mut t, b"\t");
        assert_eq!(t.cursor(), (8, 0));
        put(&mut t, b"\t");
        assert_eq!(t.cursor(), (16, 0));
        t.port_out(1, 75);
        put(&mut t, b"\t");
        assert_eq!(t.cursor(), (79, 0));
        put(&mut t, b"\t");
        assert_eq!(t.cursor(), (79, 0));
        put(&mut t, b"\r");
        assert_eq!(t.cursor(), (0, 0));
        t.port_out(1, 9);
        assert_eq!(put(&mut t, b"\n"), 0);
        assert_eq!(t.cursor(), (0, 1));
        put(&mut t, b"\x00\x01\x07\x1B\x7F");
        assert_eq!(t.cursor(), (0, 1));
        assert_eq!(ch(&t, 0, 1), ' ' as u16);
        put(&mut t, b"x");
        t.port_out(3, 5);
        assert_eq!(put(&mut t, b"\x0C"), SCROLL_CLEAR_CHARGE);
        assert_eq!(t.cursor(), (0, 0));
        assert_eq!(t.cell(0, 1), Cell { ch: 0x20, fg: 5, bg: 0 });
    }

    #[test]
    fn newline_on_last_row_scrolls() {
        let mut t = Terminal::default();
        put(&mut t, b"first");
        t.port_out(1, 0);
        t.port_out(2, 29);
        put(&mut t, b"last");
        t.port_out(4, 3);
        assert_eq!(put(&mut t, b"\n"), SCROLL_CLEAR_CHARGE);
        assert_eq!(ch(&t, 0, 28), 'l' as u16);
        assert_eq!(t.cell(0, 29), Cell { ch: 0x20, fg: 7, bg: 3 });
        assert_eq!(t.cursor(), (0, 29));
    }

    #[test]
    fn utf8_decoding() {
        let mut t = Terminal::default();
        put(&mut t, "é你".as_bytes());
        assert_eq!(ch(&t, 0, 0), 0xE9);
        assert_eq!(ch(&t, 1, 0), 0x4F60);
        assert_eq!(ch(&t, 2, 0), CONTINUATION);
        // Above U+FFFF -> FFFD, one cell.
        put(&mut t, "😀".as_bytes());
        assert_eq!(ch(&t, 3, 0), 0xFFFD);
        assert_eq!(t.cursor(), (4, 0));
    }

    #[test]
    fn malformed_utf8() {
        let cases: &[(&[u8], &[u16])] = &[
            (b"\x80", &[0xFFFD]),
            (b"\xFF", &[0xFFFD]),
            (b"\xC0\xAF", &[0xFFFD, 0xFFFD]),        // overlong lead
            (b"\xC3A", &[0xFFFD, 'A' as u16]),       // truncated, A decoded afresh
            (b"\xE4\xBDA", &[0xFFFD, 'A' as u16]),   // truncated 3-byte
            (b"\xE0\x80\x80", &[0xFFFD; 3]),         // overlong 3-byte
            (b"\xED\xA0\x80", &[0xFFFD; 3]),         // surrogate
            (b"\xF4\x90\x80\x80", &[0xFFFD; 4]),     // above U+10FFFF
            (b"\xF0\x9F\x98", &[]),                  // incomplete, still pending
            (b"\xC3\xC3\xA9", &[0xFFFD, 0xE9]),
        ];
        for (input, want) in cases {
            let mut t = Terminal::default();
            put(&mut t, input);
            let got: Vec<u16> = (0..want.len()).map(|c| ch(&t, c, 0)).collect();
            assert_eq!(&got, want, "input {input:02X?}");
            assert_eq!(t.cursor().0 as usize, want.len(), "input {input:02X?}");
        }
    }

    #[test]
    fn decoder_keeps_the_incomplete_sequence() {
        let mut t = Terminal::default();
        put(&mut t, b"\xF0\x9F\x98");
        assert_eq!((t.decoder.len, t.decoder.bytes), (3, [0xF0, 0x9F, 0x98]));
        put(&mut t, b"\x80");
        assert_eq!((t.decoder.len, t.decoder.bytes), (0, [0; 3]));
        put(&mut t, b"\xE4\xBD");
        assert_eq!((t.decoder.len, t.decoder.bytes), (2, [0xE4, 0xBD, 0]));
        put(&mut t, b"\xC3");
        assert_eq!((t.decoder.len, t.decoder.bytes), (1, [0xC3, 0, 0]), "interrupted, then decoded afresh");
        put(&mut t, b"A");
        assert_eq!(t.decoder, Utf8Decoder::default());
    }

    #[test]
    fn malformed_newline_is_still_a_newline() {
        let mut t = Terminal::default();
        put(&mut t, b"\xE4\n");
        assert_eq!(ch(&t, 0, 0), 0xFFFD);
        assert_eq!(t.cursor(), (0, 1));
    }

    #[test]
    fn cursor_and_colour_ports() {
        let mut t = Terminal::default();
        t.port_out(1, 200);
        t.port_out(2, 200);
        assert_eq!((t.port_in(1), t.port_in(2)), (79, 29));
        t.port_out(3, 0x12);
        t.port_out(4, 0x34);
        assert_eq!((t.port_in(3), t.port_in(4)), (0x12, 0x34));
        assert_eq!((t.port_in(0xB), t.port_in(0xC)), (80, 30));
        assert_eq!(t.port_in(0x0), 0xFF);
        assert_eq!(t.port_in(0xA), 0xFF);
        assert_eq!(t.port_in(0xF), 0xFF);
    }

    #[test]
    fn cell_pointer_access_and_wrap() {
        let mut t = Terminal::default();
        // Write cell (1, 0) through the pointer.
        t.port_out(6, 4);
        t.port_out(7, 0);
        for b in [b'Q', 0, 9, 1] {
            t.port_out(8, b);
        }
        assert_eq!(t.cell(1, 0), Cell { ch: 'Q' as u16, fg: 9, bg: 1 });
        assert_eq!(t.cell_pointer(), 8);
        // Wrap from 9,599 to 0.
        t.port_out(6, (9599 & 0xFF) as u8);
        t.port_out(7, (9599 >> 8) as u8);
        t.port_out(8, 0x55);
        assert_eq!(t.cell_pointer(), 0);
        assert_eq!(t.cell(79, 29).bg, 0x55);
        // Values above 9,599 are reduced modulo 9,600 when used.
        t.port_out(6, 0x84);
        t.port_out(7, 0x25); // 0x2584 = 9604 -> 4
        assert_eq!((t.port_in(6), t.port_in(7)), (0x84, 0x25));
        assert_eq!(t.port_in(8), b'Q');
        assert_eq!(t.cell_pointer(), 5);
        assert_eq!(t.port_in(8), 0);
    }

    #[test]
    fn palette_writes() {
        let mut t = Terminal::default();
        t.port_out(9, 255);
        for v in [1, 2, 3, 4, 5, 6] {
            t.port_out(0xA, v);
        }
        assert_eq!(t.palette()[255], [1, 2, 3]);
        assert_eq!(t.palette()[0], [4, 5, 6], "index wraps 255 -> 0");
        assert_eq!(t.port_in(9), 1);
        // Writing the index restarts at red.
        t.port_out(0xA, 9);
        t.port_out(9, 7);
        t.port_out(0xA, 10);
        assert_eq!(t.palette()[1][0], 9);
        assert_eq!(t.palette()[7], [10, 0xE5, 0xE5]);
    }

    #[test]
    fn commands_and_charges() {
        let mut t = Terminal::default();
        put(&mut t, b"row0\nrow1");
        t.port_out(4, 2);
        assert_eq!(t.port_out(5, 2), SCROLL_CLEAR_CHARGE);
        assert_eq!(ch(&t, 0, 0), 'r' as u16);
        assert_eq!(ch(&t, 3, 0), '1' as u16);
        assert_eq!(t.cell(0, 29), Cell { ch: 0x20, fg: 7, bg: 2 });
        assert_eq!(t.port_out(5, 3), SCROLL_CLEAR_CHARGE);
        assert_eq!(ch(&t, 3, 1), '1' as u16);
        assert_eq!(t.cell(0, 0), Cell { ch: 0x20, fg: 7, bg: 2 });
        // Clear to end of row from the cursor, free.
        t.port_out(1, 2);
        t.port_out(2, 1);
        assert_eq!(t.port_out(5, 4), 0);
        assert_eq!(ch(&t, 1, 1), 'o' as u16);
        assert_eq!(t.cell(2, 1), Cell { ch: 0x20, fg: 7, bg: 2 });
        assert_eq!(t.port_out(5, 6), 0);
        assert!(!t.cursor_visible());
        assert_eq!(t.port_out(5, 5), 0);
        assert!(t.cursor_visible());
        assert_eq!(t.port_out(5, 0x77), 0);
        assert_eq!(t.port_out(5, 1), SCROLL_CLEAR_CHARGE);
        assert_eq!(t.cursor(), (0, 0));
        assert_eq!(t.cell(5, 5), Cell { ch: 0x20, fg: 7, bg: 2 });
    }
}
