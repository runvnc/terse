//! One machine's screen in the software renderer: the terminal, the page
//! and the vector display combined in the layer order of SPEC §vector.layers (or as
//! one phosphor tube, SPEC §vector.phosphor), drawn into an image of the pixels the
//! screen covers. The same picture as `renderer.rs` draws on the GPU:
//!
//! 1. each pixel takes its cell's background colour (outside the grid, in
//!    a letterboxed window, palette entry 0);
//! 2. the page's quads, source-over in sRGB values;
//! 3. the vector shapes, source-over by coverage × opacity;
//! 4. the glyphs and the cursor: the pixel mixed with the foreground by
//!    coverage; the host's overlay rows in its own colours over everything.
//!
//! Cells, glyph pixels and screen pixels map through per-column and per-row
//! tables computed once per size, with the GPU shader's arithmetic, so a
//! glyph lands on exactly the pixels it does there.
//!
//! Only what changed since the screen was last drawn is drawn again: cells
//! whose character or colours changed, the cursor's old and new cells, the
//! page's rectangle when it scrolled or changed, the boxes of shapes that
//! moved. The damage is kept in tiles of the frame and drawn a tile row per
//! thread. An idle screen costs a comparison of its cells.

use std::collections::HashMap;
use std::sync::Arc;

use terse_core::{COLS, Primitive, ROWS, SCREEN_HEIGHT, SCREEN_WIDTH, Terminal, Vector};

use super::base::{Damage, IRect, Pixels, Pool, TILE, mix, over, pack, tile_rows};
use super::page::{PageKey, PageQuads, SoftPage};
use super::phosphor::{BloomRes, Light, Smear, Tube, linear_table};
use super::shapes::{Geom, Mode, Shape, visit};
use crate::display::{GlyphSource, SceneKey, VectorScene, grid_rect, outlined, screen_pos};
use crate::page::HostLayout;

/// A cell as drawn: its glyph's slot in the atlas and its colours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellDraw {
    pub slot: u32,
    pub fg: u8,
    pub bg: u8,
}

/// No cell (outside the grid) in a column or row table.
const OUTSIDE: u32 = u32::MAX;

/// The terminal as the passes read it: cells, glyph bitmaps and the
/// tables from pixels to cells.
pub struct Grid<'a> {
    pub cells: &'a [CellDraw],
    pub atlas: &'a [u8],
    pub cell_px: (usize, usize),
    /// Per column (row) of the target: the cell column (row) in the high
    /// 16 bits and the glyph's pixel in the low 16, or `OUTSIDE`.
    pub cols: &'a [u32],
    pub rows: &'a [u32],
    pub cursor: Option<(usize, usize)>,
    pub lin: &'a [[f32; 3]; 256],
    pub glyph_rows: &'a [bool; ROWS],
}

impl Grid<'_> {
    #[inline]
    pub fn col(&self, x: usize) -> Option<(usize, usize)> {
        let v = *self.cols.get(x)?;
        (v != OUTSIDE).then_some(((v >> 16) as usize, (v & 0xFFFF) as usize))
    }

    #[inline]
    pub fn row(&self, y: usize) -> Option<(usize, usize)> {
        let v = *self.rows.get(y)?;
        (v != OUTSIDE).then_some(((v >> 16) as usize, (v & 0xFFFF) as usize))
    }

    /// Whether a cell row has any glyph or the cursor.
    pub fn row_has_glyphs(&self, row: usize) -> bool {
        self.glyph_rows[row] || self.cursor.is_some_and(|c| c.1 == row)
    }

    /// The coverage of the glyph (or cursor) at glyph pixel (gx, gy) of
    /// cell (col, row), and the cell's foreground.
    #[inline]
    pub fn coverage(&self, row: usize, gy: usize, col: usize, gx: usize) -> (u8, u8) {
        let c = self.cells[row * COLS + col];
        let (cw, ch) = self.cell_px;
        let mut cov = self.atlas[c.slot as usize * cw * ch + gy * cw + gx];
        let underline = (ch / 10).max(1);
        if self.cursor == Some((col, row)) && gy >= ch - underline {
            cov = 255;
        }
        (cov, c.fg)
    }
}

/// The pixel tables of a target: for each of `n` pixels from `start`, the
/// cell and glyph pixel its centre falls in, as the GPU shader computes it.
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

/// Glyph bitmaps by slot: slot 0 is blank; a wide glyph takes two slots,
/// its left and right halves.
struct CellAtlas {
    slots: HashMap<(u16, bool), u32>,
    bitmaps: Vec<u8>,
    next: u32,
}

/// Slots before the atlas starts over (it needs at most two per cell, so
/// it has room for a whole grid of new glyphs several times; the bitmaps
/// grow only as slots are used).
pub(super) const ATLAS_SLOTS: u32 = 16384;

impl CellAtlas {
    fn new(cell: (u32, u32)) -> Self {
        CellAtlas { slots: HashMap::new(), bitmaps: vec![0; (cell.0 * cell.1) as usize], next: 1 }
    }
}

/// The host overlay's colours (its consent prompt): foreground codes 0
/// text, 1 key, 2 dim; background codes 0 base, 3 band (as `renderer.rs`).
fn overlay_fg(code: u8) -> u32 {
    let c = |r: f32, g: f32, b: f32| pack((r * 255.0 + 0.5) as u8, (g * 255.0 + 0.5) as u8, (b * 255.0 + 0.5) as u8);
    match code {
        1 => c(1.0, 0.80, 0.26),
        2 => c(0.66, 0.70, 0.78),
        _ => c(0.96, 0.96, 0.98),
    }
}

fn overlay_bg(code: u8) -> u32 {
    let c = |r: f32, g: f32, b: f32| pack((r * 255.0 + 0.5) as u8, (g * 255.0 + 0.5) as u8, (b * 255.0 + 0.5) as u8);
    if code == 3 { c(0.20, 0.23, 0.33) } else { c(0.09, 0.10, 0.14) }
}

/// The vector display as uploaded.
#[derive(Default)]
struct Vectors {
    uploaded: Option<SceneKey>,
    shapes: Vec<Primitive>,
    dwell: Vec<u32>,
    opcodes: Vec<u8>,
    prepared: Vec<Shape>,
    prev_shapes: Vec<Primitive>,
    prev_opcodes: Vec<u8>,
}

/// What the image shows (clean style), to find what changed.
#[derive(Default)]
struct Drawn {
    valid: bool,
    cells: Vec<CellDraw>,
    palette: Vec<[u8; 3]>,
    cursor: Option<(u32, u32)>,
    overlay: u32,
    page: PageKey,
    page_scissor: IRect,
    shapes: Vec<Shape>,
    bounds: Vec<IRect>,
}

/// Where the screen is, for one viewport.
#[derive(Clone, Copy, PartialEq)]
struct Place {
    viewport: (u32, u32),
    rect: (f32, f32, f32, f32),
    cell_px: (u32, u32),
}

pub struct SoftScreen {
    glyphs: Box<dyn GlyphSource>,
    cell_px: (u32, u32),
    atlas: CellAtlas,
    cells: Vec<CellDraw>,
    /// The terminal's bytes and palette last resolved into `cells`.
    terminal_bytes: Vec<u8>,
    /// Whether the cells were drawn with full text (SPEC §terminal.text).
    full_text: bool,
    palette: [[u8; 3]; 256],
    palette_px: [u32; 256],
    lin_palette: [[f32; 3]; 256],
    lin: [f32; 256],
    glyph_rows: [bool; ROWS],
    cursor: Option<(u32, u32)>,
    overlay: u32,
    rect: Option<(f32, f32, f32, f32)>,
    style: (bool, u8),
    no_trails: bool,
    force_phosphor: bool,
    phosphor_stale: bool,
    vectors: Vectors,
    page: SoftPage,
    place: Option<Place>,
    /// Pixel tables of the image and of the tube.
    cols: Vec<u32>,
    rows: Vec<u32>,
    tube_cols: Vec<u32>,
    tube_rows: Vec<u32>,
    /// Each cell column's and row's pixels in the image (frame coordinates).
    col_px: Vec<(i32, i32)>,
    row_px: Vec<(i32, i32)>,
    pub image: Pixels,
    /// What changed in the image since the compositor last took it.
    damage: Damage,
    drawn: Drawn,
    tube: Option<Tube>,
    /// The overlay drawn last over the phosphor picture.
    tube_overlay: u32,
    bloom: BloomRes,
    /// The sources of the machine's Surfaces (screen units), which give no
    /// phosphor light (SPEC §vector3d.presentation).
    excluded: Vec<[u16; 4]>,
}

impl SoftScreen {
    pub fn new(glyphs: Box<dyn GlyphSource>) -> Self {
        let cell_px = glyphs.cell_size();
        let lin = linear_table();
        SoftScreen {
            glyphs,
            cell_px,
            atlas: CellAtlas::new(cell_px),
            cells: vec![CellDraw::default(); COLS * ROWS],
            terminal_bytes: Vec::new(),
            full_text: true,
            palette: [[0; 3]; 256],
            palette_px: [0; 256],
            lin_palette: [[0.0; 3]; 256],
            lin,
            glyph_rows: [false; ROWS],
            cursor: None,
            overlay: 0,
            rect: None,
            style: (false, 0),
            no_trails: false,
            force_phosphor: false,
            phosphor_stale: true,
            vectors: Vectors::default(),
            page: SoftPage::default(),
            place: None,
            cols: Vec::new(),
            rows: Vec::new(),
            tube_cols: Vec::new(),
            tube_rows: Vec::new(),
            col_px: Vec::new(),
            row_px: Vec::new(),
            image: Pixels::default(),
            damage: Damage::default(),
            drawn: Drawn::default(),
            tube: None,
            tube_overlay: 0,
            bloom: BloomRes::Auto,
            excluded: Vec::new(),
        }
    }

    /// The sources of the machine's Surfaces (SPEC §vector3d.presentation): in the phosphor
    /// style they give no light (the clean style's glass is blacked out
    /// there when the Surfaces are put together over it).
    pub fn set_excluded(&mut self, rects: &[[u16; 4]]) {
        if self.excluded != rects {
            self.excluded = rects.to_vec();
        }
    }

    /// At which resolution the phosphor bloom is computed.
    pub fn set_bloom(&mut self, bloom: BloomRes) {
        if bloom != self.bloom {
            self.bloom = bloom;
            self.tube = None;
        }
    }

    pub fn glyphs_for(&self, width: u32) -> Box<dyn GlyphSource> {
        self.glyphs.resized(width)
    }

    /// Draws into `rect` (pixels) instead of the grid rectangle of the
    /// viewport, with glyphs rasterised for that size.
    pub fn set_rect(&mut self, rect: Option<(f32, f32, f32, f32)>) {
        self.rect = rect;
        if let Some(r) = rect {
            let width = crate::display::cell_width_for(r);
            if self.cell_px.0 != width {
                self.glyphs = self.glyphs.resized(width);
                self.cell_px = self.glyphs.cell_size();
                self.atlas = CellAtlas::new(self.cell_px);
                self.terminal_bytes.clear();
            }
        }
    }

    pub fn set_no_trails(&mut self, on: bool) {
        self.no_trails = on;
    }

    pub fn set_force_phosphor(&mut self, on: bool) {
        self.force_phosphor = on;
    }

    pub fn set_page_animation(&mut self, on: bool) {
        self.page.set_animate(on);
    }

    pub fn phosphor(&self) -> bool {
        self.style.0
    }

    pub fn page_anchor_at_units(&self, x: f32, y: f32) -> u16 {
        self.page.anchor_at(x, y)
    }

    pub fn take_page_figure_log(&mut self) -> Vec<String> {
        self.page.take_figure_log()
    }

    /// The grid at the glyphs' native cell size, 4:3.
    pub fn grid_size(&self) -> (u32, u32) {
        let width = COLS as u32 * self.cell_px.0;
        (width, width * SCREEN_HEIGHT as u32 / SCREEN_WIDTH as u32)
    }

    pub fn screen_rect(&self, viewport: (u32, u32)) -> (f32, f32, f32, f32) {
        self.rect.unwrap_or_else(|| grid_rect(self.grid_size(), viewport))
    }

    pub fn screen_pos(&self, viewport: (u32, u32), x: f64, y: f64) -> (u16, u16) {
        screen_pos(self.screen_rect(viewport), x, y)
    }

    pub fn row_height(&self, viewport: (u32, u32)) -> f64 {
        (self.screen_rect(viewport).3 / ROWS as f32) as f64
    }

    /// The pixels the screen draws: its rectangle in a system, the whole
    /// viewport (with the letterbox) for a screen of its own.
    pub fn area(&self) -> IRect {
        self.image.area
    }

    /// The rows the host's overlay covers, which are drawn again over the
    /// other machines' pictures (`Renderer::render_overlay`).
    pub fn overlay_rect(&self, viewport: (u32, u32)) -> IRect {
        if self.overlay == 0 {
            return IRect::EMPTY;
        }
        let (x, y, w, h) = self.screen_rect(viewport);
        let top = y + h * self.overlay as f32 / ROWS as f32;
        IRect::covering((x, y, w, h)).intersect(&IRect::of_size(viewport)).intersect(&IRect::new(i32::MIN, top.floor() as i32, i32::MAX, i32::MAX))
    }

    /// Takes the terminal (SPEC §terminal.presentation: presentation is up to the host).
    pub fn update(&mut self, terminal: &Terminal) {
        let (col, row) = terminal.cursor();
        self.cursor = terminal.cursor_visible().then_some((col as u32, row as u32));
        if terminal.palette() != &self.palette {
            self.palette = *terminal.palette();
            for (i, &[r, g, b]) in self.palette.iter().enumerate() {
                self.palette_px[i] = pack(r, g, b);
                self.lin_palette[i] = [self.lin[r as usize], self.lin[g as usize], self.lin[b as usize]];
            }
        }
        let bytes = terminal.cell_bytes();
        if bytes == self.terminal_bytes && terminal.full_text() == self.full_text {
            return;
        }
        self.terminal_bytes = bytes;
        self.full_text = terminal.full_text();
        if self.atlas.next + 2 * (COLS * ROWS) as u32 > ATLAS_SLOTS {
            self.atlas = CellAtlas::new(self.cell_px);
            // A slot now means another glyph: a cell whose slot number is
            // unchanged may still look different, so every cell is redrawn.
            self.drawn.cells.iter_mut().for_each(|c| c.slot = u32::MAX);
        }
        // SPEC §terminal.cells, §terminal.text: wide characters across two
        // cells, or, without full text, the basic repertoire and the placeholder.
        let cells = terse_core::text::draw_cells(terminal, |code, wide| self.slot_for(code, wide));
        for (i, &(slot, fg, bg)) in cells.iter().enumerate() {
            self.cells[i] = CellDraw { slot, fg, bg };
        }
        for (row, any) in self.glyph_rows.iter_mut().enumerate() {
            *any = cells[row * COLS..(row + 1) * COLS].iter().any(|c| c.0 != 0);
        }
    }

    fn slot_for(&mut self, code: u16, wide: bool) -> u32 {
        let ch = match char::from_u32(code as u32) {
            Some(c) if c != ' ' => c,
            _ => return 0,
        };
        if let Some(&slot) = self.atlas.slots.get(&(code, wide)) {
            return slot;
        }
        let cells = 1 + wide as u32;
        let (w, h) = self.cell_px;
        let mut glyph = vec![0; (cells * w * h) as usize];
        if code == terse_core::text::PLACEHOLDER_CODE {
            terse_core::text::placeholder(w, h, &mut glyph);
        } else {
            self.glyphs.rasterize(ch, cells, &mut glyph);
        }
        let slot = self.atlas.next;
        self.atlas.next += cells;
        for i in 0..cells {
            for y in 0..h {
                let start = (y * cells * w + i * w) as usize;
                self.atlas.bitmaps.extend_from_slice(&glyph[start..start + w as usize]);
            }
        }
        self.atlas.slots.insert((code, wide), slot);
        slot
    }

    /// Draws the rows from `first` on as the host's overlay.
    pub fn set_overlay(&mut self, first: Option<usize>) {
        self.overlay = first.map_or(0, |r| r.clamp(1, ROWS - 1) as u32);
    }

    /// Takes the vector display's style and lists (as `Renderer::update_vectors`).
    pub fn update_vectors(&mut self, scene: &VectorScene) {
        let vector = scene.vector;
        let persistence = vector.map_or(terse_core::vector::DEFAULT_PERSISTENCE, Vector::persistence);
        let style = (self.force_phosphor || vector.is_some_and(Vector::phosphor), persistence);
        if style.0 != self.style.0 {
            self.phosphor_stale = true;
            self.drawn.valid = false;
        }
        self.style = style;
        let outlines = outlined(style.0, vector);
        self.page.set_outlines(outlines);
        let key = scene.key(outlines);
        let v = &mut self.vectors;
        if v.uploaded == Some(key) {
            return;
        }
        v.uploaded = Some(key);
        scene.shapes(outlines, &mut v.shapes, &mut v.opcodes);
        if outlines {
            crate::phosphor::dwell_ends(&v.shapes, &mut v.dwell);
        } else {
            v.dwell.clear();
            v.dwell.resize(v.shapes.len(), 0);
        }
        v.prepared = v.shapes.iter().zip(&v.dwell).map(|(p, &d)| Shape::new(p, d)).collect();
    }

    pub fn update_page(&mut self, page: Option<&terse_core::Page>, layout: Option<Arc<HostLayout>>) {
        self.page.set(page, layout);
    }

    /// Fits the screen to `viewport`: tables, image and tube for its size.
    pub fn place(&mut self, viewport: (u32, u32)) {
        let rect = self.screen_rect(viewport);
        let place = Place { viewport, rect, cell_px: self.cell_px };
        if self.place == Some(place) {
            return;
        }
        self.place = Some(place);
        let full = IRect::of_size(viewport);
        let area = match self.rect {
            Some(r) => IRect::covering(r).intersect(&full),
            None => full,
        };
        let (x, y, w, h) = rect;
        self.cols = tables(x, w, area.x0, area.width(), COLS, self.cell_px.0);
        self.rows = tables(y, h, area.y0, area.height(), ROWS, self.cell_px.1);
        let tube = ((w.ceil() as usize).max(1), (h.ceil() as usize).max(1));
        self.tube_cols = tables(0.0, w, 0, tube.0, COLS, self.cell_px.0);
        self.tube_rows = tables(0.0, h, 0, tube.1, ROWS, self.cell_px.1);
        let spans = |table: &[u32], start: i32, n: usize| {
            let mut out = vec![(i32::MAX, i32::MIN); n];
            for (i, &v) in table.iter().enumerate() {
                if v != OUTSIDE {
                    let s = &mut out[(v >> 16) as usize];
                    s.0 = s.0.min(start + i as i32);
                    s.1 = s.1.max(start + i as i32 + 1);
                }
            }
            out
        };
        self.col_px = spans(&self.cols, area.x0, COLS);
        self.row_px = spans(&self.rows, area.y0, ROWS);
        self.image = Pixels::new(area);
        self.damage = Damage::new(viewport);
        self.damage.add(area);
        self.drawn.valid = false;
        if self.tube.as_ref().is_some_and(|t| t.size != tube) {
            self.tube = None;
        }
        self.tube_overlay = u32::MAX;
    }

    /// Takes the damage to the image since the last call.
    pub fn take_damage(&mut self, into: &mut Damage) {
        into.merge(&self.damage);
        self.damage.clear();
    }

    fn grid(&self, tube: bool) -> Grid<'_> {
        Grid {
            cells: &self.cells,
            atlas: &self.atlas.bitmaps,
            cell_px: (self.cell_px.0 as usize, self.cell_px.1 as usize),
            cols: if tube { &self.tube_cols } else { &self.cols },
            rows: if tube { &self.tube_rows } else { &self.rows },
            cursor: self.cursor.map(|(c, r)| (c as usize, r as usize)),
            lin: &self.lin_palette,
            glyph_rows: &self.glyph_rows,
        }
    }

    /// Phosphor presentation: one machine frame of light into the image
    /// (`advance`: the afterglow moves on; else only the beam is redrawn).
    /// Does nothing in the clean style.
    pub fn accumulate(&mut self, viewport: (u32, u32), advance: bool, pool: &Pool) {
        if !self.style.0 {
            return;
        }
        self.place(viewport);
        let rect = self.screen_rect(viewport);
        let size = ((rect.2.ceil() as usize).max(1), (rect.3.ceil() as usize).max(1));
        if self.tube.is_none() {
            self.tube = Some(Tube::new(size, self.bloom));
            self.phosphor_stale = true;
        }
        self.page.prepare((0.0, 0.0, rect.2, rect.3), (size.0 as u32, size.1 as u32), true);
        let persistence = if self.no_trails { 0 } else { self.style.1 };
        let clear = self.phosphor_stale;
        let v = &self.vectors;
        let mut smears = Vec::new();
        if advance && persistence > 0 {
            let smear = !clear && v.opcodes == v.prev_opcodes;
            let mut deposits = Vec::new();
            crate::phosphor::deposits(&v.prev_shapes, &v.shapes, smear, crate::phosphor::slow_decay(persistence), &mut deposits);
            smears = deposits.iter().map(|d| Smear { shape: Shape::new(&d.shape, v.dwell[d.index]), fast: d.fast, slow: d.slow }).collect();
        }
        let mut tube = self.tube.take().expect("made above");
        let excluded: Vec<(usize, usize, usize, usize)> = self
            .excluded
            .iter()
            .filter_map(|&r| {
                let (x0, y0, x1, y1) = crate::display::source_pixels((0.0, 0.0, rect.2, rect.3), r);
                let c = |v: i32, n: usize| v.clamp(0, n as i32) as usize;
                let (x0, y0, x1, y1) = (c(x0, size.0), c(y0, size.1), c(x1, size.0), c(y1, size.1));
                (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
            })
            .collect();
        let light = Light {
            excluded: &excluded,
            grid: self.grid(true),
            page: &self.page.quads,
            shapes: &self.vectors.prepared,
            smears: &smears,
            width: rect.2,
            lin: &self.lin,
            persistence,
            clear,
            advance,
        };
        tube.frame(&light, pool);
        if advance {
            self.phosphor_stale = false;
            let v = &mut self.vectors;
            v.prev_shapes.clone_from(&v.shapes);
            v.prev_opcodes.clone_from(&v.opcodes);
        }
        // The picture: the tube's top left is the rectangle's.
        let area = self.image.area;
        let (ox, oy) = (rect.0 as i32 - area.x0, rect.1 as i32 - area.y0);
        let stride = self.image.stride();
        if !self.drawn.valid || ox < 0 || oy < 0 {
            // Fresh: all black first (the letterbox stays so).
            self.image.data.fill(0);
            self.drawn.valid = true;
        }
        let rows = 0..size.1.min(area.height().saturating_sub(oy.max(0) as usize));
        let cols_fit = (ox.max(0) as usize + size.0) <= stride;
        if cols_fit && ox >= 0 && oy >= 0 {
            tube.composite(&mut self.image.data, stride, ox as usize, oy as usize, rows, pool);
        }
        self.tube = Some(tube);
        self.tube_overlay = u32::MAX;
        self.draw_overlay();
        self.damage.add(area);
    }

    /// The overlay rows over the phosphor picture.
    fn draw_overlay(&mut self) {
        if self.overlay == self.tube_overlay {
            return;
        }
        let area = self.image.area;
        if self.tube_overlay != u32::MAX && self.tube_overlay != 0 && self.tube.is_some() {
            // The overlay went or moved: the light where it was.
            let first = self.tube_overlay.min(if self.overlay == 0 { u32::MAX } else { self.overlay });
            let y0 = self.row_px[first as usize].0;
            let rect = self.screen_rect(self.place.map_or((0, 0), |p| p.viewport));
            let (ox, oy) = (rect.0 as i32 - area.x0, rect.1 as i32 - area.y0);
            let stride = self.image.stride();
            let start = (y0 - rect.1 as i32).max(0) as usize;
            let from = ((y0 - area.y0).max(0) as usize * stride).min(self.image.data.len());
            self.image.data[from..].fill(0);
            if let Some(mut tube) = self.tube.take() {
                let end = tube.size.1;
                tube.forget_shown(start..end);
                tube.composite(&mut self.image.data, stride, ox as usize, oy as usize, start..end, &Pool::new(1));
                self.tube = Some(tube);
            }
        }
        self.tube_overlay = self.overlay;
        if self.overlay == 0 {
            return;
        }
        let y0 = self.row_px[self.overlay as usize].0.max(area.y0);
        let run = IRect::new(area.x0, y0, area.x1, area.y1);
        let stride = self.image.stride();
        let mut image = std::mem::take(&mut self.image.data);
        let grid = self.grid(false);
        for y in run.y0..run.y1 {
            let Some((row, gy)) = grid.row((y - area.y0) as usize) else { continue };
            let line = &mut image[(y - area.y0) as usize * stride..][..stride];
            for (i, p) in line.iter_mut().enumerate() {
                let Some((col, gx)) = grid.col(i) else { continue };
                let (cov, _) = grid.coverage(row, gy, col, gx);
                let c = self.cells[row * COLS + col];
                *p = mix(overlay_bg(c.bg), overlay_fg(c.fg), cov as u32);
            }
        }
        self.image.data = image;
        self.damage.add(run);
    }

    /// Brings the image up to date for `viewport` (clean style: draws what
    /// changed since it was last drawn; phosphor: the light is drawn by
    /// `accumulate`, here only the overlay and a first frame).
    pub fn render(&mut self, viewport: (u32, u32), pool: &Pool) {
        self.place(viewport);
        if self.style.0 {
            if self.tube.is_none() || !self.drawn.valid {
                self.accumulate(viewport, true, pool);
            }
            self.draw_overlay();
            return;
        }
        let rect = self.screen_rect(viewport);
        let area = self.image.area;
        let scissor = IRect::covering(rect).intersect(&IRect::of_size(viewport));
        let geom = Geom::for_rect(rect);
        // What changed since the image was drawn.
        let page_key = self.page.key(rect, false);
        let page_changed = !self.drawn.valid || page_key != self.drawn.page;
        if page_changed {
            self.page.prepare(rect, viewport, false);
        }
        let mut damage = Damage::new(viewport);
        if !self.drawn.valid || self.palette.as_slice() != self.drawn.palette.as_slice() {
            damage.add(area);
        } else {
            for row in 0..ROWS {
                let now = &self.cells[row * COLS..(row + 1) * COLS];
                let was = &self.drawn.cells[row * COLS..(row + 1) * COLS];
                if now == was {
                    continue;
                }
                let first = (0..COLS).find(|&c| now[c] != was[c]).unwrap_or(0);
                let last = (0..COLS).rev().find(|&c| now[c] != was[c]).unwrap_or(0);
                let (x0, x1) = (self.col_px[first].0, self.col_px[last].1);
                let (y0, y1) = self.row_px[row];
                damage.add(IRect::new(x0, y0, x1, y1));
            }
            if self.cursor != self.drawn.cursor {
                for (c, r) in [self.cursor, self.drawn.cursor].into_iter().flatten() {
                    let (x0, x1) = self.col_px[c as usize];
                    let (y0, y1) = self.row_px[r as usize];
                    damage.add(IRect::new(x0, y0, x1, y1));
                }
            }
            if self.overlay != self.drawn.overlay {
                let first = [self.overlay, self.drawn.overlay].into_iter().filter(|&r| r > 0).min().unwrap_or(0);
                damage.add(IRect::new(area.x0, self.row_px[first as usize].0, area.x1, area.y1));
            }
            if page_changed {
                damage.add(self.drawn.page_scissor);
                damage.add(self.page.quads.scissor);
            }
            let (now, was) = (&self.vectors.prepared, &self.drawn.shapes);
            if now != was {
                for i in 0..now.len().max(was.len()) {
                    if now.get(i) != was.get(i) {
                        if let Some(s) = now.get(i) {
                            damage.add(s.bounds(&geom, false));
                        }
                        if let Some(b) = self.drawn.bounds.get(i) {
                            damage.add(*b);
                        }
                    }
                }
            }
        }
        // Remember what is drawn now.
        self.drawn.valid = true;
        self.drawn.cells.clone_from(&self.cells);
        self.drawn.palette.clear();
        self.drawn.palette.extend_from_slice(&self.palette);
        self.drawn.cursor = self.cursor;
        self.drawn.overlay = self.overlay;
        if page_changed {
            self.drawn.page = self.page.key(rect, false);
            self.drawn.page_scissor = self.page.quads.scissor;
        }
        if self.drawn.shapes != self.vectors.prepared {
            self.drawn.shapes.clone_from(&self.vectors.prepared);
            self.drawn.bounds = self.drawn.shapes.iter().map(|s| s.bounds(&geom, false)).collect();
        }
        if damage.is_empty() {
            return;
        }
        // Draw the damaged tiles, a tile row per thread.
        let stride = self.image.stride();
        let mut image = std::mem::take(&mut self.image.data);
        let scene = Clean {
            grid: self.grid(false),
            area,
            scissor,
            geom,
            page: &self.page.quads,
            shapes: &self.drawn.shapes,
            bounds: &self.drawn.bounds,
            palette: &self.palette_px,
            overlay: self.overlay as usize,
            col_px: &self.col_px,
        };
        let rows = damage.rows();
        let mut work = Vec::new();
        let mut bands = tile_rows(&mut image, stride, area).into_iter().peekable();
        for (ty, runs) in rows {
            while bands.peek().is_some_and(|(y0, _)| y0.div_euclid(TILE) < ty as i32) {
                bands.next();
            }
            let Some((y0, band)) = bands.next_if(|(y0, _)| y0.div_euclid(TILE) == ty as i32) else { continue };
            let runs: Vec<IRect> = runs.iter().map(|r| r.intersect(&area)).filter(|r| !r.is_empty()).collect();
            if !runs.is_empty() {
                work.push((y0, band, runs));
            }
        }
        pool.each(work, |(y0, band, runs)| {
            for run in runs {
                scene.draw(band, y0, stride, run);
            }
        });
        self.image.data = image;
        self.damage.merge(&damage);
    }
}

/// The clean style's passes for a run of pixels.
struct Clean<'a> {
    grid: Grid<'a>,
    area: IRect,
    scissor: IRect,
    geom: Geom,
    page: &'a PageQuads,
    shapes: &'a [Shape],
    bounds: &'a [IRect],
    palette: &'a [u32; 256],
    overlay: usize,
    /// Each cell column's pixels (frame coordinates).
    col_px: &'a [(i32, i32)],
}

impl Clean<'_> {
    /// The cells of a pixel row (frame coordinates) that meet `run`: the
    /// cell column and its pixels, clipped to the run.
    fn cells(&self, run: IRect) -> Vec<(usize, i32, i32)> {
        (0..COLS)
            .filter_map(|c| {
                let (x0, x1) = self.col_px[c];
                let (x0, x1) = (x0.max(run.x0), x1.min(run.x1));
                (x0 < x1).then_some((c, x0, x1))
            })
            .collect()
    }

    /// Draws `run` (frame coordinates, within `band`'s rows starting at
    /// `y0`) from scratch, cell by cell: a blank cell is a fill.
    fn draw(&self, band: &mut [u32], y0: i32, stride: usize, run: IRect) {
        let g = &self.grid;
        let ax = self.area.x0;
        let page = !self.page.is_empty() && !run.intersect(&self.page.scissor).is_empty();
        let clip = run.intersect(&self.scissor);
        let shapes: Vec<usize> = (0..self.shapes.len()).filter(|&i| !self.bounds[i].intersect(&clip).is_empty()).collect();
        let layered = page || !shapes.is_empty();
        let cells = self.cells(run);
        // Columns of the run outside the grid (a letterbox): palette 0.
        let (gx0, gx1) = cells.first().zip(cells.last()).map_or((run.x1, run.x1), |(f, l)| (f.1, l.2));
        let cursor = g.cursor;
        let glyph = |r: usize, c: usize| g.cells[r * COLS + c].slot != 0 || cursor == Some((c, r));
        let at = |x: i32| (x - ax) as usize;
        // 1. Backgrounds (and, if nothing lies between, the glyphs at once).
        for y in run.y0..run.y1 {
            let line = &mut band[(y - y0) as usize * stride..][..stride];
            let Some((r, gy)) = g.row((y - self.area.y0) as usize) else {
                line[at(run.x0)..at(run.x1)].fill(self.palette[0]);
                continue;
            };
            line[at(run.x0)..at(gx0)].fill(self.palette[0]);
            line[at(gx1)..at(run.x1)].fill(self.palette[0]);
            let overlay = self.overlay > 0 && r >= self.overlay;
            for &(c, x0, x1) in &cells {
                let cell = g.cells[r * COLS + c];
                let bg = self.palette[cell.bg as usize];
                let span = &mut line[at(x0)..at(x1)];
                if layered || overlay || !glyph(r, c) {
                    span.fill(bg);
                    continue;
                }
                let fg = self.palette[cell.fg as usize];
                for (i, p) in span.iter_mut().enumerate() {
                    let (_, gx) = g.col(at(x0) + i).expect("in the grid");
                    let (cov, _) = g.coverage(r, gy, c, gx);
                    *p = if cov == 0 { bg } else { mix(bg, fg, cov as u32) };
                }
            }
        }
        if layered {
            // 2. The page.
            if page {
                self.page.draw_clean(band, ax, y0, stride, run);
            }
            // 3. The shapes, in list order.
            for &i in &shapes {
                let s = &self.shapes[i];
                let [r, gg, b, a] = s.rgba();
                if a == 0 {
                    continue;
                }
                let c = [r as f32, gg as f32, b as f32];
                let alpha = a as f32 / 255.0;
                visit(s, &self.geom, Mode::Cover, clip, |x, y, v| {
                    let p = &mut band[(y - y0) as usize * stride + (x - ax) as usize];
                    *p = over(*p, c, alpha * v);
                });
            }
            // 4. Glyphs and the cursor.
            for y in run.y0..run.y1 {
                let Some((r, gy)) = g.row((y - self.area.y0) as usize) else { continue };
                if self.overlay > 0 && r >= self.overlay {
                    continue;
                }
                let line = &mut band[(y - y0) as usize * stride..][..stride];
                for &(c, x0, x1) in &cells {
                    if !glyph(r, c) {
                        continue;
                    }
                    let fg = self.palette[g.cells[r * COLS + c].fg as usize];
                    for x in at(x0)..at(x1) {
                        let (_, gx) = g.col(x).expect("in the grid");
                        let (cov, _) = g.coverage(r, gy, c, gx);
                        if cov > 0 {
                            line[x] = mix(line[x], fg, cov as u32);
                        }
                    }
                }
            }
        }
        // The host's overlay over everything, in its own colours.
        if self.overlay > 0 {
            for y in run.y0..run.y1 {
                let Some((r, gy)) = g.row((y - self.area.y0) as usize) else { continue };
                if r < self.overlay {
                    continue;
                }
                let line = &mut band[(y - y0) as usize * stride..][..stride];
                for &(c, x0, x1) in &cells {
                    let cell = g.cells[r * COLS + c];
                    let (bg, fg) = (overlay_bg(cell.bg), overlay_fg(cell.fg));
                    for x in at(x0)..at(x1) {
                        let (_, gx) = g.col(x).expect("in the grid");
                        let (cov, _) = g.coverage(r, gy, c, gx);
                        line[x] = mix(bg, fg, cov as u32);
                    }
                }
            }
        }
    }
}
