//! Device 2, Terminal (SPEC §terminal). Its operations return how many clears and
//! scrolls they did, which the machine charges for when a command does them
//! (SPEC §charges; inside a `write` they are not charged).
//!
//! A scroll or a clear costs a host time proportional to one row at most
//! (SPEC §terminal.presentation): the rows are a ring (`top` is the stored row shown first), and
//! a row that a scroll or clear blanked is only filled when something is
//! written to it.

use crate::wide::is_wide;

pub const COLS: usize = 80;
pub const ROWS: usize = 30;
pub const CELL_BYTES: usize = 4;
pub const CELLS: usize = COLS * ROWS; // 2,400
pub const CELL_MEMORY: usize = CELLS * CELL_BYTES; // 9,600

/// The right cell of a wide character (SPEC §terminal.cells).
pub const CONTINUATION: u16 = 0xFFFF;
const REPLACEMENT: u16 = 0xFFFD;
const SPACE: u16 = 0x0020;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: u16,
    pub fg: u8,
    pub bg: u8,
}

/// Incremental UTF-8 decoder for `write` (SPEC §terminal.put).
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

#[derive(Clone, Debug)]
pub struct Terminal {
    /// The rows as stored: stored row `(top + r) % ROWS` is row `r`.
    cells: Vec<u8>,
    top: usize,
    /// Per stored row: the cell every cell of it holds, if it was blanked
    /// and not written since (its bytes in `cells` are then stale).
    blank: [Option<[u8; CELL_BYTES]>; ROWS],
    /// Clears so far, and the cell the last one blanked every row with: a
    /// stored row whose `stamp` is below `clears` holds `clear_cell`.
    clears: u64,
    clear_cell: [u8; CELL_BYTES],
    stamp: [u64; ROWS],
    col: u8,
    row: u8,
    fg: u8,
    bg: u8,
    cursor_visible: bool,
    /// SPEC §terminal.put: the next printed character starts a new row first.
    pending_wrap: bool,
    palette: [[u8; 3]; 256],
    decoder: Utf8Decoder,
}

impl Default for Terminal {
    fn default() -> Self {
        // SPEC §terminal.reset
        let mut t = Terminal {
            cells: vec![0; CELL_MEMORY],
            top: 0,
            blank: [None; ROWS],
            clears: 0,
            clear_cell: [0; CELL_BYTES],
            stamp: [0; ROWS],
            col: 0,
            row: 0,
            fg: 7,
            bg: 0,
            cursor_visible: true,
            pending_wrap: false,
            palette: xterm_palette(),
            decoder: Utf8Decoder::default(),
        };
        t.fill_rows(0..ROWS);
        t
    }
}

impl PartialEq for Terminal {
    fn eq(&self, other: &Self) -> bool {
        let (mut a, mut b) = (Vec::new(), Vec::new());
        self.write_state(&mut a);
        other.write_state(&mut b);
        a == b
    }
}

impl Eq for Terminal {}

const ROW_BYTES: usize = COLS * CELL_BYTES;

impl Terminal {
    /// The stored row that is row `row`.
    #[inline(always)]
    fn stored(&self, row: usize) -> usize {
        let p = self.top + row;
        if p >= ROWS { p - ROWS } else { p }
    }

    /// The cell all of stored row `p` holds, if it is blank.
    #[inline(always)]
    fn blank_cell(&self, p: usize) -> Option<[u8; CELL_BYTES]> {
        if self.stamp[p] < self.clears { Some(self.clear_cell) } else { self.blank[p] }
    }

    /// Row `row`'s bytes, or the one cell all of it holds.
    fn row_bytes(&self, row: usize) -> Result<&[u8], [u8; CELL_BYTES]> {
        let p = self.stored(row);
        match self.blank_cell(p) {
            Some(c) => Err(c),
            None => Ok(&self.cells[p * ROW_BYTES..(p + 1) * ROW_BYTES]),
        }
    }

    /// Row `row`'s bytes, to change: a blanked row is filled first (the
    /// only work proportional to a row that a scroll or clear causes).
    #[inline(always)]
    fn row_mut(&mut self, row: usize) -> &mut [u8] {
        let p = self.stored(row);
        let blank = self.blank_cell(p);
        self.blank[p] = None;
        self.stamp[p] = self.clears;
        let r = &mut self.cells[p * ROW_BYTES..(p + 1) * ROW_BYTES];
        if let Some(c) = blank {
            for cell in r.chunks_exact_mut(CELL_BYTES) {
                cell.copy_from_slice(&c);
            }
        }
        r
    }

    pub fn cell(&self, col: usize, row: usize) -> Cell {
        let c = match self.row_bytes(row) {
            Ok(r) => r[col * CELL_BYTES..(col + 1) * CELL_BYTES].try_into().unwrap(),
            Err(c) => c,
        };
        Cell { ch: u16::from_le_bytes([c[0], c[1]]), fg: c[2], bg: c[3] }
    }

    /// The cell memory, 4 bytes per cell (SPEC §terminal.cells).
    pub fn cell_bytes(&self) -> Vec<u8> {
        let mut out = vec![0; CELL_MEMORY];
        self.cells_read(0, &mut out);
        out
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

    /// Cells, cursor, colours, palette and the put-character decoder
    /// (STATE.md, terminal section).
    pub fn write_state(&self, out: &mut Vec<u8>) {
        let at = out.len();
        out.resize(at + CELL_MEMORY, 0);
        self.cells_read(0, &mut out[at..]);
        out.extend_from_slice(&[self.col, self.row, self.fg, self.bg]);
        out.extend_from_slice(&[self.cursor_visible as u8, self.pending_wrap as u8]);
        out.extend(self.palette.iter().flatten());
        out.push(self.decoder.len);
        out.extend_from_slice(&self.decoder.bytes);
    }

    /// The terminal section as `write_state` wrote it (STATE.md 4). The
    /// incomplete sequence is decoded again from its bytes.
    pub fn restore(&mut self, r: &mut crate::snapshot::Reader) -> Result<(), crate::snapshot::RestoreError> {
        use crate::snapshot::bad;
        self.cells.copy_from_slice(r.take(CELL_MEMORY)?);
        self.top = 0;
        self.blank = [None; ROWS];
        self.stamp = [self.clears; ROWS];
        let [col, row, fg, bg] = r.array()?;
        if col as usize >= COLS || row as usize >= ROWS {
            return Err(bad("cursor"));
        }
        (self.col, self.row, self.fg, self.bg) = (col, row, fg, bg);
        self.cursor_visible = r.flag()?;
        self.pending_wrap = r.flag()?;
        for entry in &mut self.palette {
            *entry = r.array()?;
        }
        let k = r.u8()? as usize;
        let bytes: [u8; 3] = r.array()?;
        if k > 3 {
            return Err(bad("incomplete UTF-8 sequence"));
        }
        let mut d = Utf8Decoder::default();
        for &b in &bytes[..k] {
            if d.feed(b) != [None, None] {
                return Err(bad("incomplete UTF-8 sequence"));
            }
        }
        if d.len as usize != k || d.bytes != bytes {
            return Err(bad("incomplete UTF-8 sequence"));
        }
        self.decoder = d;
        Ok(())
    }

    /// Put character (SPEC §terminal.put) for each byte of a UTF-8 stream; a sequence
    /// may continue in the next call. Returns the clears and scrolls done.
    pub fn write(&mut self, bytes: &[u8]) -> u32 {
        let mut ops = 0;
        for &b in bytes {
            ops += self.put_byte(b);
        }
        ops
    }

    /// One byte of put character; returns the clears and scrolls done.
    #[inline(always)]
    fn put_byte(&mut self, b: u8) -> u32 {
        // Printable ASCII with nothing pending: the common case.
        if (0x20..0x7F).contains(&b) && self.decoder.needed == 0 && !self.pending_wrap {
            let col = self.col as usize;
            if col < COLS - 1 {
                let (fg, bg) = (self.fg, self.bg);
                let r = self.row_mut(self.row as usize);
                let i = col * CELL_BYTES;
                let here = u16::from_le_bytes([r[i], r[i + 1]]);
                let next = u16::from_le_bytes([r[i + 4], r[i + 5]]);
                // Not part of a wide pair (no neighbour to blank, SPEC §terminal.put
                // step 3) and no wrap: just the cell and the cursor.
                if here != CONTINUATION && next != CONTINUATION {
                    r[i..i + CELL_BYTES].copy_from_slice(&[b, 0, fg, bg]);
                    self.col += 1;
                    return 0;
                }
            }
        }
        let mut ops = 0;
        for cp in self.decoder.feed(b).into_iter().flatten() {
            ops += self.put_char(cp);
        }
        ops
    }

    /// Moves the cursor, clamping to the grid (SPEC §terminal.calls); clears pending wrap.
    pub fn set_cursor(&mut self, col: u32, row: u32) {
        self.col = col.min(COLS as u32 - 1) as u8;
        self.row = row.min(ROWS as u32 - 1) as u8;
        self.pending_wrap = false;
    }

    /// Sets the current colours.
    pub fn set_colours(&mut self, fg: u8, bg: u8) {
        self.fg = fg;
        self.bg = bg;
    }

    /// Copies cells from cell `first` (0-2399) into `out` (4 bytes each,
    /// SPEC §terminal.cells), wrapping from the last cell to the first.
    pub fn cells_read(&self, first: usize, out: &mut [u8]) {
        let mut at = first * CELL_BYTES % CELL_MEMORY;
        let mut done = 0;
        while done < out.len() {
            // To the end of the row at most.
            let (row, off) = (at / ROW_BYTES, at % ROW_BYTES);
            let n = (out.len() - done).min(ROW_BYTES - off);
            let dst = &mut out[done..done + n];
            match self.row_bytes(row) {
                Ok(r) => dst.copy_from_slice(&r[off..off + n]),
                Err(c) => {
                    // A piece starts on a cell (count × 4 bytes from cell `first`).
                    let mut cells = dst.chunks_exact_mut(CELL_BYTES);
                    for cell in &mut cells {
                        cell.copy_from_slice(&c);
                    }
                    let rest = cells.into_remainder();
                    let k = rest.len();
                    rest.copy_from_slice(&c[..k]);
                }
            }
            done += n;
            at = (at + n) % CELL_MEMORY;
        }
    }

    /// Writes cells from cell `first`, the reverse of `cells_read`. The
    /// bytes are not checked (SPEC §terminal.calls).
    pub fn cells_write(&mut self, first: usize, data: &[u8]) {
        let mut at = first * CELL_BYTES % CELL_MEMORY;
        let mut done = 0;
        while done < data.len() {
            let (row, off) = (at / ROW_BYTES, at % ROW_BYTES);
            let n = (data.len() - done).min(ROW_BYTES - off);
            self.row_mut(row)[off..off + n].copy_from_slice(&data[done..done + n]);
            done += n;
            at = (at + n) % CELL_MEMORY;
        }
    }

    /// Sets palette entries from `first` (wrapping from 255 to 0) from
    /// red, green, blue triples.
    pub fn palette_set(&mut self, first: usize, rgb: &[u8]) {
        for (i, c) in rgb.chunks_exact(3).enumerate() {
            self.palette[(first + i) % 256] = [c[0], c[1], c[2]];
        }
    }

    /// A command (SPEC §terminal.commands); returns the clears and scrolls done (0 or 1).
    pub fn command(&mut self, value: u8) -> u32 {
        match value {
            0x01 => self.clear(),
            0x02 => self.scroll_up(),
            0x03 => {
                self.top = self.stored(ROWS - 1);
                self.fill_rows(0..1);
                1
            }
            0x04 => {
                // Starting on the right half of a wide pair breaks the pair:
                // its left half becomes a space and keeps its colours, as in
                // SPEC §terminal.put step 3.
                let (col, row) = (self.col as usize, self.row as usize);
                if col > 0 && self.is_pair(col - 1, row) {
                    let i = (col - 1) * CELL_BYTES;
                    self.row_mut(row)[i..i + 2].copy_from_slice(&SPACE.to_le_bytes());
                }
                for c in col..COLS {
                    self.set_cell(c, row, SPACE);
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

    /// SPEC §terminal.put; returns the clears and scrolls done.
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

    /// Prints one character (SPEC §terminal.put steps 1-4); returns the scrolls done.
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
    /// keeps its colours (SPEC §terminal.put step 3).
    fn write_cells(&mut self, col: usize, chars: &[u16]) {
        let row = self.row as usize;
        let end = col + chars.len();
        let broken = [
            (col > 0 && self.is_pair(col - 1, row)).then(|| col - 1),
            self.is_pair(end - 1, row).then_some(end),
        ];
        for c in broken.into_iter().flatten() {
            let i = c * CELL_BYTES;
            self.row_mut(row)[i..i + 2].copy_from_slice(&SPACE.to_le_bytes());
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

    fn clear(&mut self) -> u32 {
        self.fill_rows(0..ROWS);
        self.col = 0;
        self.row = 0;
        self.pending_wrap = false;
        1
    }

    fn scroll_up(&mut self) -> u32 {
        self.top = self.stored(1);
        self.fill_rows(ROWS - 1..ROWS);
        1
    }

    fn set_cell(&mut self, col: usize, row: usize, ch: u16) {
        let i = col * CELL_BYTES;
        let [lo, hi] = ch.to_le_bytes();
        let (fg, bg) = (self.fg, self.bg);
        self.row_mut(row)[i..i + CELL_BYTES].copy_from_slice(&[lo, hi, fg, bg]);
    }

    /// Blanks the rows with spaces in the current colours (filled later,
    /// when written).
    fn fill_rows(&mut self, rows: std::ops::Range<usize>) {
        let [lo, hi] = SPACE.to_le_bytes();
        let cell = [lo, hi, self.fg, self.bg];
        if rows == (0..ROWS) {
            self.clears += 1;
            self.clear_cell = cell;
            return;
        }
        for row in rows {
            let p = self.stored(row);
            self.blank[p] = Some(cell);
            self.stamp[p] = self.clears;
        }
    }
}

/// The xterm 256-colour palette (SPEC §terminal.reset).
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

    /// Feeds the bytes through the decoder and put character, without the
    /// fast path of `write`.
    fn put(t: &mut Terminal, s: &[u8]) -> u32 {
        let mut ops = 0;
        for &b in s {
            for cp in t.decoder.feed(b).into_iter().flatten() {
                ops += t.put_char(cp);
            }
        }
        ops
    }

    /// The old register writes, as calls: 1 column, 2 row, 3 foreground,
    /// 4 background, 5 command.
    trait Regs {
        fn port_out(&mut self, reg: u8, v: u8) -> u32;
    }
    impl Regs for Terminal {
        fn port_out(&mut self, reg: u8, v: u8) -> u32 {
            let ((col, row), (fg, bg)) = (self.cursor(), self.colors());
            match reg {
                1 => self.set_cursor(v as u32, row as u32),
                2 => self.set_cursor(col as u32, v as u32),
                3 => self.set_colours(v, bg),
                4 => self.set_colours(fg, v),
                5 => return self.command(v),
                _ => unreachable!(),
            }
            0
        }
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
        assert_eq!(put(&mut t, b"Y"), 1);
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
        assert_eq!(put(&mut t, "好".as_bytes()), 1);
        assert_eq!((ch(&t, 0, 29), ch(&t, 1, 29), ch(&t, 79, 28)), (0x597D, CONTINUATION, 0x20));
        assert_eq!(t.cursor(), (2, 29));

        // With pending wrap set, column 79 keeps the character just printed.
        t.port_out(1, 79);
        t.port_out(2, 5);
        put(&mut t, "z好".as_bytes());
        assert_eq!(ch(&t, 79, 5), 'z' as u16);
        assert_eq!((ch(&t, 0, 6), ch(&t, 1, 6)), (0x597D, CONTINUATION));
    }

    /// SPEC §terminal.put step 1: a wide character at column 79 over the right half
    /// of a wide pair at 78-79: column 79 becomes a space in the current
    /// colours, and the pair's left half at 78 a space keeping its colours.
    #[test]
    fn wide_at_column_79_over_a_pair_blanks_both_halves() {
        let mut t = Terminal::default();
        t.set_colours(1, 2);
        t.set_cursor(78, 0);
        t.write("你".as_bytes());
        t.set_cursor(79, 0);
        t.set_colours(3, 4);
        t.write("好".as_bytes());
        assert_eq!(t.cell(78, 0), Cell { ch: 0x20, fg: 1, bg: 2 });
        assert_eq!(t.cell(79, 0), Cell { ch: 0x20, fg: 3, bg: 4 });
        assert_eq!((t.cell(0, 1).ch, t.cell(1, 1).ch), (0x597D, CONTINUATION));
    }

    /// SPEC §terminal.commands: commands 02h-06h leave the cursor and pending wrap as they
    /// are; 01h moves the cursor to (0, 0) and clears pending wrap.
    #[test]
    fn commands_keep_the_cursor_and_pending_wrap() {
        for c in 2..=6u8 {
            let mut t = Terminal::default();
            t.set_cursor(70, 9);
            t.write(&[b'x'; 10]);
            assert_eq!((t.cursor(), t.pending_wrap()), ((79, 9), true));
            t.command(c);
            assert_eq!((t.cursor(), t.pending_wrap()), ((79, 9), true), "command {c:02X}h");
        }
        let mut t = Terminal::default();
        t.set_cursor(70, 9);
        t.write(&[b'x'; 10]);
        t.command(1);
        assert_eq!((t.cursor(), t.pending_wrap()), ((0, 0), false));
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
        // A stray continuation code written with cells_write.
        t.cells_write(COLS, &[0xFF, 0xFF, 7, 0]);
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
        assert_eq!(put(&mut t, b"\x0C"), 1);
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
        assert_eq!(put(&mut t, b"\n"), 1);
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
    fn cursor_and_colours() {
        let mut t = Terminal::default();
        t.set_cursor(200, u32::MAX);
        assert_eq!(t.cursor(), (79, 29));
        t.set_colours(0x12, 0x34);
        assert_eq!(t.colors(), (0x12, 0x34));
    }

    #[test]
    fn cells_read_write_and_wrap() {
        let mut t = Terminal::default();
        t.cells_write(1, &[b'Q', 0, 9, 1]);
        assert_eq!(t.cell(1, 0), Cell { ch: 'Q' as u16, fg: 9, bg: 1 });
        // From the last cell, wrapping to the first.
        t.cells_write(CELLS - 1, &[b'A', 0, 1, 2, b'B', 0, 3, 4]);
        assert_eq!(t.cell(79, 29), Cell { ch: 'A' as u16, fg: 1, bg: 2 });
        assert_eq!(t.cell(0, 0), Cell { ch: 'B' as u16, fg: 3, bg: 4 });
        let mut out = [0u8; 12];
        t.cells_read(CELLS - 1, &mut out);
        assert_eq!(out, [b'A', 0, 1, 2, b'B', 0, 3, 4, b'Q', 0, 9, 1]);
        // The whole grid, from the middle: every cell once.
        let mut all = vec![0u8; CELL_MEMORY];
        t.cells_read(1200, &mut all);
        assert_eq!(&all[..CELL_MEMORY / 2], &t.cell_bytes()[CELL_MEMORY / 2..]);
        assert_eq!(&all[CELL_MEMORY / 2..], &t.cell_bytes()[..CELL_MEMORY / 2]);
        // Not checked: a continuation code is written as given.
        t.cells_write(2, &[0xFF, 0xFF, 0, 0]);
        assert_eq!(t.cell(2, 0).ch, CONTINUATION);
    }

    #[test]
    fn palette_writes() {
        let mut t = Terminal::default();
        t.palette_set(255, &[1, 2, 3, 4, 5, 6]);
        assert_eq!(t.palette()[255], [1, 2, 3]);
        assert_eq!(t.palette()[0], [4, 5, 6], "index wraps 255 -> 0");
        assert_eq!(t.palette()[1], [0xCD, 0, 0]);
        t.palette_set(7, &[10, 11, 12]);
        assert_eq!(t.palette()[7], [10, 11, 12]);
    }

    #[test]
    fn clear_to_end_of_row_breaks_a_wide_pair() {
        let mut t = Terminal::default();
        t.set_colours(1, 2);
        t.write("a你".as_bytes());
        t.set_colours(3, 4);
        t.set_cursor(2, 0);
        t.command(0x04);
        assert_eq!(t.cell(1, 0), Cell { ch: 0x20, fg: 1, bg: 2 }, "left half blanked, colours kept");
        assert_eq!(t.cell(2, 0), Cell { ch: 0x20, fg: 3, bg: 4 });
        assert_eq!(t.cell(0, 0).ch, 'a' as u16);
    }

    /// Scrolls and clears turn the ring and blank rows lazily; what the
    /// program and the state see is the grid as SPEC §terminal says.
    #[test]
    fn scrolls_and_clears_keep_the_grid() {
        let mut t = Terminal::default();
        let text: String = (0..45).map(|i| format!("line {i}\n")).collect();
        assert_eq!(t.write(text.as_bytes()), 16);
        let rows: Vec<String> = t.text().lines().map(str::to_string).collect();
        assert_eq!((rows[0].as_str(), rows[28].as_str(), rows[29].as_str()), ("line 16", "line 44", ""));
        t.set_colours(3, 4);
        assert_eq!(t.command(3), 1, "scroll down");
        assert_eq!(t.cell(5, 0), Cell { ch: 0x20, fg: 3, bg: 4 });
        assert_eq!(t.cell(0, 1).ch, 'l' as u16);
        // cells_read and the state across the ring's seam and blank rows.
        let bytes = t.cell_bytes();
        for (i, c) in bytes.chunks(4).enumerate() {
            let cell = t.cell(i % COLS, i / COLS);
            assert_eq!(c, [cell.ch as u8, (cell.ch >> 8) as u8, cell.fg, cell.bg]);
        }
        let mut odd = vec![0u8; 4 * 101];
        t.cells_read(2399 - 50, &mut odd);
        assert_eq!(&odd[..204], &bytes[(2399 - 50) * 4..]);
        assert_eq!(&odd[204..], &bytes[..200]);
        // The state restores to an equal terminal.
        let mut state = Vec::new();
        t.write_state(&mut state);
        let mut u = Terminal::default();
        u.restore(&mut crate::snapshot::Reader::new(&state)).unwrap();
        assert_eq!(t, u);
        assert_eq!(t.write(b"\x0Cx"), 1);
        assert_eq!((t.cell(0, 0).ch, t.cell(1, 0), t.cell(79, 29)), ('x' as u16, Cell { ch: 0x20, fg: 3, bg: 4 }, Cell { ch: 0x20, fg: 3, bg: 4 }));
        t.cells_write(2399, &[b'A', 0, 1, 2, b'B', 0, 3, 4]);
        assert_eq!((t.cell(79, 29).ch, t.cell(0, 0).ch, t.cell(1, 0).ch), ('A' as u16, 'B' as u16, 0x20));
    }

    #[test]
    fn bulk_write_matches_byte_at_a_time() {
        // The fast path for ASCII must agree with put character everywhere:
        // next to wide pairs, at the end of rows, with pending wrap.
        let text = "ab你cd\n好xyz\tq\u{7F}\r".repeat(40) + &"0123456789".repeat(300);
        let mut a = Terminal::default();
        let mut b = Terminal::default();
        for start in [0u32, 77, 78, 79] {
            a.set_cursor(start, 3);
            b.set_cursor(start, 3);
            a.write("你你你".as_bytes());
            put(&mut b, "你你你".as_bytes());
            a.set_cursor(start, 3);
            b.set_cursor(start, 3);
            assert_eq!(a.write(text.as_bytes()), put(&mut b, text.as_bytes()));
            assert_eq!(a, b, "from column {start}");
        }
    }

    #[test]
    fn commands_and_charges() {
        let mut t = Terminal::default();
        put(&mut t, b"row0\nrow1");
        t.port_out(4, 2);
        assert_eq!(t.port_out(5, 2), 1);
        assert_eq!(ch(&t, 0, 0), 'r' as u16);
        assert_eq!(ch(&t, 3, 0), '1' as u16);
        assert_eq!(t.cell(0, 29), Cell { ch: 0x20, fg: 7, bg: 2 });
        assert_eq!(t.port_out(5, 3), 1);
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
        assert_eq!(t.port_out(5, 1), 1);
        assert_eq!(t.cursor(), (0, 0));
        assert_eq!(t.cell(5, 5), Cell { ch: 0x20, fg: 7, bg: 2 });
    }
}
