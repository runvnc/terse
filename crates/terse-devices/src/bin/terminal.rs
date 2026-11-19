//! `terse-dev-terminal`: the terminal (SPEC 8, slot 2) as a driver
//! (DEVICES.md): terse-core's `Terminal` for the state, and two layers
//! drawn as terse-host's software renderer draws the grid: the cell
//! backgrounds (z 10) and the characters and cursor (z 40).

use std::collections::HashMap;

use terse_bus::driver::{Begin, CallIn, Device, Inputs, LayerFile, LayerOut, Mem, Present, PresentOut, Reply, Session};
use terse_bus::proto::{CallEntry, DeviceInfo, LayerDecl, NO_ARG, access, rule, z};
use terse_core::machine::{charge, copy_charge};
use terse_core::terminal::{CELL_BYTES, CELLS};
use terse_core::{COLS, CONTINUATION, ERR_BUFFER, ERR_UNKNOWN, ROWS, Terminal};
use terse_devices::{rect, union};
use terse_host::display::GlyphSource;
use terse_host::soft::base::IRect;

const BG: u8 = 0;
const FG: u8 = 1;
const OUTSIDE: u32 = u32::MAX;

/// The call table (DEVICES.md 5.1).
fn table() -> Vec<CallEntry> {
    let e = CallEntry::plain;
    vec![
        // write: the whole buffer, at most 4,096 bytes; its length back, 2 per byte.
        e(0).buffer(0, 1, 1, 0, charge::MAX_WRITE, access::READ | access::ROM).batch(rule::ARGUMENT, 1, 0, 0, 0).per_byte(charge::WRITE_BYTE as u16),
        e(1).batch(rule::CONSTANT, 0, 0, 0, 0).charge_when(0, 1, 3, charge::SCROLL_CLEAR),
        e(2).batch(rule::CONSTANT, 0, 0, 0, 0),
        e(3).sets_a1(),
        e(4).batch(rule::CONSTANT, 0, 0, 0, 0),
        e(5).sets_a1(),
        e(6).buffer(1, 2, 4, 0, (CELLS * CELL_BYTES) as u32, access::WRITE).check(0, CELLS as u32 - 1),
        e(7).buffer(1, 2, 4, 0, (CELLS * CELL_BYTES) as u32, access::READ | access::ROM).check(0, CELLS as u32 - 1).batch(rule::ARGUMENT, 2, 0, 0, 0).per4(),
        e(8).buffer(1, 2, 3, 0, 768, access::READ | access::ROM).check(0, 255).batch(rule::ARGUMENT, 2, 0, 0, 0).per4(),
        e(9).sets_a1().batch(rule::CONSTANT, 0, COLS as u32, ROWS as u32, 0),
    ]
}

/// The two layers' pixels and what they show.
struct Drawn {
    picture: (u32, u32),
    screen: (f32, f32, f32, f32),
    bg: LayerFile,
    fg: LayerFile,
    cols: Vec<u32>,
    rows: Vec<u32>,
    col_px: Vec<(i32, i32)>,
    row_px: Vec<(i32, i32)>,
    cells: Vec<(u32, u8, u8)>,
    palette: [[u8; 3]; 256],
    cursor: Option<(usize, usize)>,
}

struct Glyphs {
    source: Box<dyn GlyphSource>,
    cell: (u32, u32),
    slots: HashMap<(u16, bool), u32>,
    bitmaps: Vec<u8>,
    next: u32,
}

impl Glyphs {
    fn slot(&mut self, code: u16, wide: bool) -> u32 {
        let ch = match char::from_u32(code as u32) {
            Some(c) if c != ' ' => c,
            _ => return 0,
        };
        if let Some(&s) = self.slots.get(&(code, wide)) {
            return s;
        }
        let cells = 1 + wide as u32;
        let (w, h) = self.cell;
        let mut glyph = vec![0; (cells * w * h) as usize];
        self.source.rasterize(ch, cells, &mut glyph);
        let slot = self.next;
        self.next += cells;
        for i in 0..cells {
            for y in 0..h {
                let start = (y * cells * w + i * w) as usize;
                self.bitmaps.extend_from_slice(&glyph[start..start + w as usize]);
            }
        }
        self.slots.insert((code, wide), slot);
        slot
    }
}

struct TerminalDriver {
    t: Terminal,
    glyphs: Option<Glyphs>,
    drawn: Option<Drawn>,
}

/// As the software renderer's `tables`: per pixel of `start..start + n`,
/// the cell and glyph pixel its centre falls in.
fn tables(origin: f32, size: f32, start: i32, n: usize, cells: usize, cell_px: u32) -> Vec<u32> {
    (0..n)
        .map(|i| {
            let pos = (start + i as i32) as f32 + 0.5;
            let rel = (pos - origin) / size;
            if !(0.0..1.0).contains(&rel) {
                return OUTSIDE;
            }
            let g = rel * cells as f32;
            let cell = (g.floor() as u32).min(cells as u32 - 1);
            let px = (((g - g.floor()) * cell_px as f32) as u32).min(cell_px - 1);
            cell << 16 | px
        })
        .collect()
}

fn spans(table: &[u32], n: usize) -> Vec<(i32, i32)> {
    let mut out = vec![(i32::MAX, i32::MIN); n];
    for (i, &v) in table.iter().enumerate() {
        if v != OUTSIDE {
            let s = &mut out[(v >> 16) as usize];
            s.0 = s.0.min(i as i32);
            s.1 = s.1.max(i as i32 + 1);
        }
    }
    out
}

impl TerminalDriver {
    /// The cells as drawn: glyph slot, foreground, background.
    fn cells(&mut self) -> Vec<(u32, u8, u8)> {
        let g = self.glyphs.as_mut().unwrap();
        if g.next + 2 * CELLS as u32 > 4096 {
            g.slots.clear();
            g.bitmaps.truncate((g.cell.0 * g.cell.1) as usize);
            g.next = 1;
        }
        let mut out = vec![(0, 0, 0); CELLS];
        for row in 0..ROWS {
            let mut col = 0;
            while col < COLS {
                let cell = self.t.cell(col, row);
                let wide = cell.ch != CONTINUATION && col + 1 < COLS && self.t.cell(col + 1, row).ch == CONTINUATION;
                let slot = if cell.ch == CONTINUATION { 0 } else { g.slot(cell.ch, wide) };
                for half in 0..1 + wide as usize {
                    let c = self.t.cell(col + half, row);
                    out[row * COLS + col + half] = (if slot == 0 { 0 } else { slot + half as u32 }, c.fg, c.bg);
                }
                col += 1 + wide as usize;
            }
        }
        out
    }

    fn draw(&mut self, p: &Present, session: &Session) -> Result<PresentOut, String> {
        if self.glyphs.is_none() {
            return Ok(PresentOut::default());
        }
        let cells = self.cells();
        let cursor = self.t.cursor_visible().then(|| (self.t.cursor().0 as usize, self.t.cursor().1 as usize));
        let palette = *self.t.palette();
        let cell_px = self.glyphs.as_ref().unwrap().cell;
        let mut fresh = false;
        if self.drawn.as_ref().is_none_or(|d| d.picture != p.picture || d.screen != p.screen) {
            let (x, y, w, h) = p.screen;
            let cols = tables(x, w, 0, p.picture.0 as usize, COLS, cell_px.0);
            let rows = tables(y, h, 0, p.picture.1 as usize, ROWS, cell_px.1);
            self.drawn = Some(Drawn {
                picture: p.picture,
                screen: p.screen,
                bg: LayerFile::create(session.dir(), 2, BG, p.picture)?,
                fg: LayerFile::create(session.dir(), 2, FG, p.picture)?,
                col_px: spans(&cols, COLS),
                row_px: spans(&rows, ROWS),
                cols,
                rows,
                cells: Vec::new(),
                palette,
                cursor: None,
            });
            fresh = true;
        }
        let d = self.drawn.as_mut().unwrap();
        // What changed: whole picture, or rows of cells and the cursor.
        let mut damage: Vec<IRect> = Vec::new();
        if fresh || d.palette != palette {
            damage.push(IRect::of_size(p.picture));
        } else {
            for row in 0..ROWS {
                let (now, was) = (&cells[row * COLS..(row + 1) * COLS], &d.cells[row * COLS..(row + 1) * COLS]);
                if now == was {
                    continue;
                }
                let first = (0..COLS).find(|&c| now[c] != was[c]).unwrap_or(0);
                let last = (0..COLS).rev().find(|&c| now[c] != was[c]).unwrap_or(0);
                damage.push(IRect::new(d.col_px[first].0, d.row_px[row].0, d.col_px[last].1, d.row_px[row].1));
            }
            if cursor != d.cursor {
                for (c, r) in [cursor, d.cursor].into_iter().flatten() {
                    damage.push(IRect::new(d.col_px[c].0, d.row_px[r].0, d.col_px[c].1, d.row_px[r].1));
                }
            }
        }
        let pal: Vec<u32> = palette.iter().map(|&[r, g, b]| 0xFF00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32).collect();
        let glyphs = self.glyphs.as_ref().unwrap();
        let (cw, ch) = (cell_px.0 as usize, cell_px.1 as usize);
        let underline = (ch / 10).max(1);
        let w = p.picture.0 as usize;
        let bg_px = d.bg.pixels();
        for r in &damage {
            for y in r.y0.max(0)..r.y1.min(p.picture.1 as i32) {
                let rv = d.rows[y as usize];
                for x in r.x0.max(0)..r.x1.min(p.picture.0 as i32) {
                    let cv = d.cols[x as usize];
                    let i = y as usize * w + x as usize;
                    if rv == OUTSIDE || cv == OUTSIDE {
                        bg_px[i] = pal[0];
                        continue;
                    }
                    bg_px[i] = pal[cells[(rv >> 16) as usize * COLS + (cv >> 16) as usize].2 as usize];
                }
            }
        }
        let fg_px = d.fg.pixels();
        for r in &damage {
            for y in r.y0.max(0)..r.y1.min(p.picture.1 as i32) {
                let rv = d.rows[y as usize];
                for x in r.x0.max(0)..r.x1.min(p.picture.0 as i32) {
                    let cv = d.cols[x as usize];
                    let i = y as usize * w + x as usize;
                    if rv == OUTSIDE || cv == OUTSIDE {
                        fg_px[i] = 0;
                        continue;
                    }
                    let (row, gy, col, gx) = ((rv >> 16) as usize, (rv & 0xFFFF) as usize, (cv >> 16) as usize, (cv & 0xFFFF) as usize);
                    let (slot, fg, _) = cells[row * COLS + col];
                    let mut cov = glyphs.bitmaps[slot as usize * cw * ch + gy * cw + gx];
                    if cursor == Some((col, row)) && gy >= ch - underline {
                        cov = 255;
                    }
                    let [cr, cg, cb] = palette[fg as usize];
                    fg_px[i] = if cov == 0 { 0 } else { terse_devices::premul(cr, cg, cb, cov) };
                }
            }
        }
        d.cells = cells;
        d.palette = palette;
        d.cursor = cursor;
        let all = damage.iter().fold(IRect::EMPTY, |a, &b| union(a, b));
        let dmg = if all.is_empty() { Vec::new() } else { vec![rect(all)] };
        Ok(PresentOut {
            layers: vec![
                LayerOut { id: BG, new_file: fresh.then(|| d.bg.path.clone()), damage: dmg.clone() },
                LayerOut { id: FG, new_file: fresh.then(|| d.fg.path.clone()), damage: dmg },
            ],
            properties: Vec::new(),
        })
    }
}

impl Device for TerminalDriver {
    fn info(&mut self, session: &Session) -> DeviceInfo {
        let mut native = (0, 0);
        let mut layers = Vec::new();
        if session.presents() {
            match terse_host::glyphs::SystemGlyphs::new(terse_host::FONT_PX, None) {
                Ok(g) => {
                    let cell = g.cell_size();
                    let width = COLS as u32 * cell.0;
                    native = (width as u16, (width * 480 / 640) as u16);
                    layers = vec![LayerDecl { id: BG, z: z::TERMINAL_BACKGROUNDS }, LayerDecl { id: FG, z: z::TERMINAL_CHARACTERS }];
                    self.glyphs = Some(Glyphs { source: Box::new(g), cell, slots: HashMap::new(), bitmaps: vec![0; (cell.0 * cell.1) as usize], next: 1 });
                }
                Err(e) => eprintln!("terse-dev-terminal: {e}; not drawing"),
            }
        }
        DeviceInfo { version: 0, name: "terse-dev-terminal (Rust)".into(), native, layers, table: table() }
    }

    fn reset(&mut self) {
        self.t = Terminal::default();
    }

    fn call(&mut self, c: &CallIn, mem: &mut dyn Mem) -> Reply {
        let t = &mut self.t;
        let [a0, a1, a2, ..] = c.args;
        let mut charge_units = 0;
        let mut r1 = None;
        let r0 = match c.function {
            0 if c.buf.len() == a1 as usize => {
                t.write(c.buf);
                charge_units = a1 * charge::WRITE_BYTE;
                a1
            }
            0 => ERR_BUFFER,
            1 => {
                if a0 < 0x100 {
                    charge_units = t.command(a0 as u8) * charge::SCROLL_CLEAR;
                }
                0
            }
            2 => {
                t.set_cursor(a0, a1);
                0
            }
            3 => {
                let (col, row) = t.cursor();
                r1 = Some(row as u32);
                col as u32
            }
            4 => {
                let (fg, bg) = t.colors();
                t.set_colours(u8::try_from(a0).unwrap_or(fg), u8::try_from(a1).unwrap_or(bg));
                0
            }
            5 => {
                let (fg, bg) = t.colors();
                r1 = Some(bg as u32);
                fg as u32
            }
            6 if (a0 as usize) < CELLS && a2 as usize <= CELLS => {
                let mut out = vec![0; a2 as usize * CELL_BYTES];
                t.cells_read(a0 as usize, &mut out);
                if !out.is_empty() {
                    mem.write(a1, &out);
                }
                charge_units = copy_charge(out.len());
                a2
            }
            7 if (a0 as usize) < CELLS && c.buf.len() == a2 as usize * CELL_BYTES => {
                t.cells_write(a0 as usize, c.buf);
                charge_units = copy_charge(c.buf.len());
                a2
            }
            8 if a0 < 256 && c.buf.len() == a2 as usize * 3 => {
                t.palette_set(a0 as usize, c.buf);
                charge_units = copy_charge(c.buf.len());
                a2
            }
            6..=8 => ERR_BUFFER,
            9 => {
                r1 = Some(ROWS as u32);
                COLS as u32
            }
            _ => ERR_UNKNOWN,
        };
        Reply { a0: r0, a1: r1, charge: charge_units }
    }

    fn begin(&mut self, _: &Begin, _: &mut dyn Mem) -> Inputs {
        Inputs::default()
    }

    fn end(&mut self, _: u32, _: bool) {}

    fn state(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.t.write_state(&mut out);
        out
    }

    fn restore(&mut self, bytes: &[u8]) -> bool {
        let mut t = Terminal::default();
        let mut r = terse_core::snapshot::Reader::new(bytes);
        if t.restore(&mut r).is_err() || r.remaining() != 0 {
            return false;
        }
        let mut again = Vec::new();
        t.write_state(&mut again);
        if again != bytes {
            return false;
        }
        self.t = t;
        true
    }

    fn present(&mut self, p: &Present, session: &Session) -> PresentOut {
        self.draw(p, session).unwrap_or_else(|e| {
            eprintln!("terse-dev-terminal: {e}");
            PresentOut::default()
        })
    }
}

fn main() -> std::process::ExitCode {
    let _ = NO_ARG;
    terse_bus::driver::run(&mut TerminalDriver { t: Terminal::default(), glyphs: None, drawn: None }, "terse-dev-terminal")
}
