//! `terse-dev-terminal`: the terminal (SPEC §terminal, slot 2) as a driver
//! (DEVICES.md): per machine, terse-core's `Terminal` for the state, and
//! two layers drawn as terse-host's software renderer draws the grid: the
//! cell backgrounds (z 10) and the characters and cursor (z 40), with
//! glyphs rasterised for the cell width the core asks for (as terse-host
//! does for a machine shown in a tile or on a panel). It subscribes to the
//! core's status line, which it shows over the bottom row as terse-host
//! shows a fault, and to light mode (the phosphor style, DEVICES.md 7.1),
//! in which the backgrounds are left out and the characters are light:
//! their colour straight, their coverage in the alpha.

use std::collections::HashMap;

use terse_bus::driver::{Begin, CallIn, Device, Inputs, LayerFile, LayerOut, Mem, Present, PresentOut, Reply, Session};
use terse_bus::proto::{CallEntry, DeviceInfo, LayerDecl, Open, SlotInfo, access, property, rule, z};
use terse_core::machine::{charge, copy_charge};
use terse_core::terminal::{CELL_BYTES, CELLS};
use terse_core::{COLS, ERR_BUFFER, ERR_UNKNOWN, ROWS, Terminal};
use terse_devices::{rect, union};
use terse_host::display::GlyphSource;
use terse_host::soft::base::IRect;

const SLOT: u8 = 2;
const BG: u8 = 0;
const FG: u8 = 1;
const OUTSIDE: u32 = u32::MAX;
/// Glyph slots an atlas holds before it starts again: room for a whole
/// screen of new wide glyphs after the ones it has (terse-host's software
/// renderer starts again whenever the grid changes, which costs the same
/// pixels but rasterises every glyph again).
const ATLAS_SLOTS: u32 = 16384;

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
    cell_px: (u32, u32),
    atlas: u64,
    bg: LayerFile,
    fg: LayerFile,
    cols: Vec<u32>,
    rows: Vec<u32>,
    col_px: Vec<(i32, i32)>,
    row_px: Vec<(i32, i32)>,
    cells: Vec<(u32, u8, u8)>,
    palette: [[u8; 3]; 256],
    cursor: Option<(usize, usize)>,
    /// Drawn as light (`core.light`).
    light: bool,
}

/// Glyphs of one cell size, shared by every machine drawn at it.
struct Glyphs {
    source: Box<dyn GlyphSource>,
    cell: (u32, u32),
    slots: HashMap<(u16, bool), u32>,
    bitmaps: Vec<u8>,
    next: u32,
    /// Changes when the atlas starts again (slots mean other glyphs).
    generation: u64,
}

impl Glyphs {
    fn new(source: Box<dyn GlyphSource>, generation: u64) -> Self {
        let cell = source.cell_size();
        Glyphs { source, cell, slots: HashMap::new(), bitmaps: vec![0; (cell.0 * cell.1) as usize], next: 1, generation }
    }

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
        if code == terse_core::text::PLACEHOLDER_CODE {
            terse_core::text::placeholder(w, h, &mut glyph);
        } else {
            self.source.rasterize(ch, cells, &mut glyph);
        }
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

/// One machine's terminal.
struct Inst {
    t: Terminal,
    /// The core's status line (a fault), shown over the bottom row.
    status: String,
    /// Light mode (`core.light`).
    light: bool,
    drawn: Option<Drawn>,
}

struct TerminalDriver {
    /// Atlases by cell width (0: the native glyphs, there if the core
    /// presents).
    atlases: HashMap<u32, Glyphs>,
    /// The fonts, loaded when the driver starts (SPEC §responsiveness item 5).
    fonts: terse_host::fonts::TerminalFonts,
    generations: u64,
    inst: HashMap<u32, Inst>,
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

/// The terminal as shown: with the status line over the bottom row, as
/// terse-host shows a fault (colours 15 on 1, the row cleared, the cursor
/// hidden).
fn shown<'a>(t: &'a Terminal, status: &str) -> std::borrow::Cow<'a, Terminal> {
    if status.is_empty() {
        return std::borrow::Cow::Borrowed(t);
    }
    let mut t = t.clone();
    t.set_colours(15, 1);
    t.set_cursor(0, ROWS as u32 - 1);
    t.command(0x04);
    t.write(format!(" {status}").as_bytes());
    t.command(0x06);
    std::borrow::Cow::Owned(t)
}

/// The cells as drawn: glyph slot, foreground, background.
fn cells(t: &Terminal, g: &mut Glyphs, generations: &mut u64) -> Vec<(u32, u8, u8)> {
    if g.next + 2 * CELLS as u32 > ATLAS_SLOTS {
        g.slots.clear();
        g.bitmaps.truncate((g.cell.0 * g.cell.1) as usize);
        g.next = 1;
        *generations += 1;
        g.generation = *generations;
    }
    terse_core::text::draw_cells(t, |code, wide| g.slot(code, wide))
}

impl TerminalDriver {
    fn draw(&mut self, instance: u32, p: &Present, session: &Session) -> Result<PresentOut, String> {
        // Cell width 0: the native glyphs; else glyphs resized to it.
        let Some(native) = self.atlases.get(&0) else { return Ok(PresentOut::default()) };
        if !self.atlases.contains_key(&p.cell) {
            let source = native.source.resized(p.cell.max(2));
            self.generations += 1;
            self.atlases.insert(p.cell, Glyphs::new(source, self.generations));
        }
        let g = self.atlases.get_mut(&p.cell).unwrap();
        let i = self.inst.get_mut(&instance).ok_or("no such instance")?;
        let t = shown(&i.t, &i.status);
        let cells = cells(&t, g, &mut self.generations);
        let cursor = t.cursor_visible().then(|| (t.cursor().0 as usize, t.cursor().1 as usize));
        let palette = *t.palette();
        let cell_px = g.cell;
        let mut fresh = false;
        if i.drawn.as_ref().is_none_or(|d| d.picture != p.picture || d.screen != p.screen || d.cell_px != cell_px) {
            let (x, y, w, h) = p.screen;
            let cols = tables(x, w, 0, p.picture.0 as usize, COLS, cell_px.0);
            let rows = tables(y, h, 0, p.picture.1 as usize, ROWS, cell_px.1);
            i.drawn = Some(Drawn {
                picture: p.picture,
                screen: p.screen,
                cell_px,
                atlas: g.generation,
                bg: LayerFile::create(session.dir(), instance, SLOT, BG, p.picture)?,
                fg: LayerFile::create(session.dir(), instance, SLOT, FG, p.picture)?,
                col_px: spans(&cols, COLS),
                row_px: spans(&rows, ROWS),
                cols,
                rows,
                cells: Vec::new(),
                palette,
                cursor: None,
                light: i.light,
            });
            fresh = true;
        }
        let d = i.drawn.as_mut().unwrap();
        // What changed: the whole picture, or rows of cells and the cursor.
        let mut damage: Vec<IRect> = Vec::new();
        let light = i.light;
        if fresh || d.palette != palette || d.atlas != g.generation || d.light != light {
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
        let (cw, ch) = (cell_px.0 as usize, cell_px.1 as usize);
        let underline = (ch / 10).max(1);
        let w = p.picture.0 as usize;
        let bg_px = d.bg.pixels();
        for r in &damage {
            for y in r.y0.max(0)..r.y1.min(p.picture.1 as i32) {
                let rv = d.rows[y as usize];
                for x in r.x0.max(0)..r.x1.min(p.picture.0 as i32) {
                    let cv = d.cols[x as usize];
                    let at = y as usize * w + x as usize;
                    if light {
                        bg_px[at] = 0;
                        continue;
                    }
                    if rv == OUTSIDE || cv == OUTSIDE {
                        bg_px[at] = pal[0];
                        continue;
                    }
                    bg_px[at] = pal[cells[(rv >> 16) as usize * COLS + (cv >> 16) as usize].2 as usize];
                }
            }
        }
        let fg_px = d.fg.pixels();
        for r in &damage {
            for y in r.y0.max(0)..r.y1.min(p.picture.1 as i32) {
                let rv = d.rows[y as usize];
                for x in r.x0.max(0)..r.x1.min(p.picture.0 as i32) {
                    let cv = d.cols[x as usize];
                    let at = y as usize * w + x as usize;
                    if rv == OUTSIDE || cv == OUTSIDE {
                        fg_px[at] = 0;
                        continue;
                    }
                    let (row, gy, col, gx) = ((rv >> 16) as usize, (rv & 0xFFFF) as usize, (cv >> 16) as usize, (cv & 0xFFFF) as usize);
                    let (slot, fg, _) = cells[row * COLS + col];
                    let mut cov = g.bitmaps[slot as usize * cw * ch + gy * cw + gx];
                    if cursor == Some((col, row)) && gy >= ch - underline {
                        cov = 255;
                    }
                    let [cr, cg, cb] = palette[fg as usize];
                    fg_px[at] = match (cov, light) {
                        (0, _) => 0,
                        (_, true) => (cov as u32) << 24 | (cr as u32) << 16 | (cg as u32) << 8 | cb as u32,
                        (_, false) => terse_devices::premul(cr, cg, cb, cov),
                    };
                }
            }
        }
        d.cells = cells;
        d.palette = palette;
        d.cursor = cursor;
        d.atlas = g.generation;
        d.light = light;
        let all = damage.iter().fold(IRect::EMPTY, |a, &b| union(a, b));
        let dmg = if all.is_empty() { Vec::new() } else { vec![rect(all)] };
        Ok(PresentOut {
            layers: vec![
                LayerOut { slot: SLOT, id: BG, new_file: fresh.then(|| d.bg.path.clone()), damage: dmg.clone() },
                LayerOut { slot: SLOT, id: FG, new_file: fresh.then(|| d.fg.path.clone()), damage: dmg },
            ],
        })
    }
}

impl Device for TerminalDriver {
    fn info(&mut self, session: &Session) -> DeviceInfo {
        let mut native = (0, 0);
        let mut layers = Vec::new();
        if session.presents() {
            match terse_host::glyphs::TermGlyphs::new(terse_host::FONT_PX, &self.fonts) {
                Ok(g) => {
                    let cell = g.cell_size();
                    let width = COLS as u32 * cell.0;
                    native = (width as u16, (width * 480 / 640) as u16);
                    layers = vec![LayerDecl { id: BG, z: z::TERMINAL_BACKGROUNDS }, LayerDecl { id: FG, z: z::TERMINAL_CHARACTERS }];
                    self.generations += 1;
                    self.atlases.insert(0, Glyphs::new(Box::new(g), self.generations));
                }
                Err(e) => eprintln!("terse-dev-terminal: {e}; not drawing"),
            }
        }
        DeviceInfo {
            name: "terse-dev-terminal (Rust)".into(),
            native,
            // Device 10, full text (SPEC §terminal.text): a capability with
            // no calls and no layers; a machine that has it is drawn with
            // every character the fonts have.
            slots: vec![
                SlotInfo { slot: SLOT, version: 0, layers, table: table() },
                SlotInfo { slot: terse_core::text::FULL_TEXT, version: 0, layers: Vec::new(), table: Vec::new() },
            ]
            .into_iter()
            .filter(|s| session.asks(s.slot))
            .collect(),
            publishes: Vec::new(),
            subscribes: vec![property::STATUS.into(), property::LIGHT.into()],
            services: Vec::new(),
            ..Default::default()
        }
    }

    fn open(&mut self, instance: u32, open: &Open, _: &Session) -> Result<(), String> {
        let full_text = open.listed.iter().any(|&(s, _)| s == terse_core::text::FULL_TEXT);
        self.inst.insert(instance, Inst { t: Terminal::with_full_text(full_text), status: String::new(), light: false, drawn: None });
        Ok(())
    }

    fn close(&mut self, instance: u32) {
        self.inst.remove(&instance);
    }

    fn reset(&mut self, instance: u32) {
        if let Some(i) = self.inst.get_mut(&instance) {
            i.t = Terminal::with_full_text(i.t.full_text());
        }
    }

    fn call(&mut self, instance: u32, c: &CallIn, mem: &mut dyn Mem) -> Reply {
        let Some(i) = self.inst.get_mut(&instance) else { return Reply { a0: ERR_UNKNOWN, ..Default::default() } };
        let t = &mut i.t;
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

    fn begin(&mut self, _: u32, _: &Begin, _: &mut dyn Mem) -> Inputs {
        Inputs::default()
    }

    fn end(&mut self, _: u32, _: u32, _: bool) {}

    fn state(&self, instance: u32, _: u8) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(i) = self.inst.get(&instance) {
            i.t.write_state(&mut out);
        }
        out
    }

    fn restore(&mut self, instance: u32, _: u8, bytes: &[u8]) -> Option<usize> {
        let i = self.inst.get_mut(&instance)?;
        let mut t = Terminal::default();
        let mut r = terse_core::snapshot::Reader::new(bytes);
        t.restore(&mut r).ok()?;
        let n = r.position();
        let mut again = Vec::new();
        t.write_state(&mut again);
        (again == bytes[..n]).then_some(())?;
        i.t = t;
        Some(n)
    }

    fn set(&mut self, instance: u32, name: &str, value: &[u8]) {
        let Some(i) = self.inst.get_mut(&instance) else { return };
        match name {
            property::STATUS => i.status = String::from_utf8_lossy(value).into_owned(),
            property::LIGHT => i.light = value.first().is_some_and(|&v| v != 0),
            _ => {}
        }
    }

    fn present(&mut self, instance: u32, p: &Present, session: &Session) -> PresentOut {
        self.draw(instance, p, session).unwrap_or_else(|e| {
            eprintln!("terse-dev-terminal: {e}");
            PresentOut::default()
        })
    }
}

fn main() -> std::process::ExitCode {
    if terse_bus::driver::version("terse-dev-terminal") {
        return std::process::ExitCode::SUCCESS;
    }
    // `--config <dir>`: the fonts `<dir>/fonts` names (README "Fonts"); else
    // the built-in font. Other arguments are ignored (DEVICES.md 2.1).
    let args: Vec<String> = std::env::args().collect();
    let config = args.iter().position(|a| a == "--config").and_then(|i| args.get(i + 1)).map(std::path::PathBuf::from);
    let fonts = terse_host::fonts::TerminalFonts::load(config.as_deref());
    for n in &fonts.notes {
        eprintln!("terse-dev-terminal: fonts: {n}");
    }
    let mut d = TerminalDriver { atlases: HashMap::new(), fonts, generations: 0, inst: HashMap::new() };
    terse_bus::driver::run(&mut d, "terse-dev-terminal")
}
