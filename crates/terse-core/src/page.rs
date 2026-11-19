//! Device 7, Page (SPEC 11): typeset proportional text.
//!
//! The device only validates and copies the document and keeps the
//! rectangle, the scroll value and the background. Laying the text out is
//! the host's business: [`parse`] turns a document into paragraphs of styled
//! runs (the one decoder every host uses), and [`reference_layout`] is the
//! deterministic layout of SPEC 11.6 that a host without fonts (`terse run`,
//! the tests) uses. [`PageWatch`] decides when a host reports the height
//! (event `08h`), and [`PageInput`] does that and the anchor events for a
//! headless host.

use std::fmt::Write as _;

use crate::system::{Event, SCREEN_HEIGHT, SCREEN_WIDTH};
use crate::wide::is_wide;

/// Longest document a submit accepts (SPEC 11.2).
pub const MAX_DOCUMENT: usize = 65_536;
/// Results of `submit`.
pub const RESULT_DONE: u32 = 0;
pub const RESULT_REJECTED: u32 = 1;
/// The background after reset: opaque black (`r | g << 8 | b << 16 | a << 24`).
pub const DEFAULT_BACKGROUND: u32 = 0xFF00_0000;
/// Largest height event `08h` can carry (24 bits).
pub const MAX_REPORTED_HEIGHT: u32 = 0xFF_FFFF;
/// Most `locate` calls a frame takes (SPEC 11.2); later ones are rejected.
pub const MAX_LOCATES: usize = 8;
/// Most locates a host keeps waiting for a layout (SPEC 11.5); older ones
/// are dropped.
pub const MAX_PENDING_LOCATES: usize = 8;

/// Document opcodes (SPEC 11.3).
pub mod op {
    pub const END: u8 = 0x00;
    pub const TEXT: u8 = 0x01;
    pub const FONT: u8 = 0x02;
    pub const SIZE: u8 = 0x03;
    pub const COLOUR: u8 = 0x04;
    pub const PARAGRAPH: u8 = 0x05;
    pub const RULE: u8 = 0x06;
    pub const ANCHOR: u8 = 0x07;
    pub const FIGURE: u8 = 0x08;
    pub const TABLE: u8 = 0x09;
    pub const CELL: u8 = 0x0A;
    pub const END_TABLE: u8 = 0x0B;
}

/// Path opcodes inside a Figure (SPEC 11.3).
pub mod path_op {
    pub const FILL: u8 = 0x01;
    pub const STROKE: u8 = 0x02;
    pub const MOVE: u8 = 0x03;
    pub const LINE: u8 = 0x04;
    pub const QUAD: u8 = 0x05;
    pub const CUBIC: u8 = 0x06;
    pub const CLOSE: u8 = 0x07;
}

/// Font faces (SPEC 11.3).
pub mod face {
    pub const SANS: u8 = 0;
    pub const SERIF: u8 = 1;
    pub const MONO: u8 = 2;
}

/// Sizes are clamped to this range (SPEC 11.3).
pub const MIN_SIZE: u8 = 6;
pub const MAX_SIZE: u8 = 96;
/// Initial text state (SPEC 11.3).
pub const DEFAULT_SIZE: u8 = 14;
pub const DEFAULT_COLOUR: [u8; 3] = [229, 229, 229];
/// Height of a rule paragraph in screen units (SPEC 11.3).
pub const RULE_HEIGHT: u32 = 8;
/// Tables (SPEC 11.3, 11.4): at most this many columns, cells in one
/// table and cells in one document.
pub const MAX_TABLE_COLUMNS: u8 = 8;
pub const MAX_TABLE_CELLS: usize = 1_024;
pub const MAX_DOCUMENT_CELLS: usize = 4_096;
/// A cell's padding in screen units: left and right, top and bottom.
pub const CELL_PAD_X: u32 = 6;
pub const CELL_PAD_Y: u32 = 3;
/// Columns of a table too wide for the rectangle are narrowed to a common
/// cap, never below this many screen units.
pub const MIN_COLUMN_CAP: u32 = 48;
/// Faint rules (between the rows of a table) are drawn at this opacity.
pub const FAINT_RULE_ALPHA: u8 = 85;

/// The rectangle in screen units: x, y, width, height.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Region {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Region {
    /// Whether the page is shown at all (a width or height of 0 hides it).
    pub fn visible(&self) -> bool {
        self.w > 0 && self.h > 0
    }
}

/// The device state (SPEC 11.7): the document, the rectangle, the scroll
/// value and the background.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    /// `None`: no document (after reset and `clear`).
    document: Option<Vec<u8>>,
    region: Region,
    scroll: u32,
    background: u32,
    /// Changes at every accepted submit and every clear (not machine state;
    /// hosts use it to know when to lay out and report the height).
    generation: u64,
    /// The `locate` calls of this frame: the document generation at the
    /// call and the anchor (not machine state: the answers are inputs).
    locates: Vec<(u64, u16)>,
}

impl Default for Page {
    fn default() -> Self {
        Page {
            document: None,
            region: Region::default(),
            scroll: 0,
            background: DEFAULT_BACKGROUND,
            generation: 0,
            locates: Vec::new(),
        }
    }
}

impl Page {
    /// Replaces the document with `bytes` and sets the scroll value to 0 if
    /// it is 1 to 65,536 bytes long; else returns [`RESULT_REJECTED`] and
    /// keeps everything. The caller checks the buffer.
    pub fn submit(&mut self, bytes: &[u8]) -> u32 {
        if bytes.is_empty() || bytes.len() > MAX_DOCUMENT {
            return RESULT_REJECTED;
        }
        let doc = self.document.get_or_insert_with(Vec::new);
        doc.clear();
        doc.extend_from_slice(bytes);
        self.scroll = 0;
        self.generation += 1;
        RESULT_DONE
    }

    /// No document.
    pub fn clear(&mut self) {
        self.document = None;
        self.generation += 1;
    }

    /// Sets the rectangle, clamped so it lies on the screen.
    pub fn set_region(&mut self, x: u32, y: u32, w: u32, h: u32) {
        let (sw, sh) = (SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32);
        let x = x.min(sw);
        let y = y.min(sh);
        self.region = Region { x: x as u16, y: y as u16, w: w.min(sw - x) as u16, h: h.min(sh - y) as u16 };
    }

    pub fn set_scroll(&mut self, y: u32) {
        self.scroll = y;
    }

    pub fn set_background(&mut self, rgba: u32) {
        self.background = rgba;
    }

    /// `locate` (SPEC 11.2): asks for where anchor `id` is laid out in the
    /// current document. [`RESULT_REJECTED`] for an id of 0 or above
    /// 65,535, with no document, or after [`MAX_LOCATES`] in this frame.
    pub fn locate(&mut self, id: u32) -> u32 {
        if id == 0 || id > u16::MAX as u32 || self.document.is_none() || self.locates.len() >= MAX_LOCATES {
            return RESULT_REJECTED;
        }
        self.locates.push((self.generation, id as u16));
        RESULT_DONE
    }

    /// The `locate` calls of the frame just run, in order: the document
    /// generation at each call and the anchor.
    pub fn locates(&self) -> &[(u64, u16)] {
        &self.locates
    }

    /// A new frame: the last frame's locates are the host's now (the
    /// machine calls this at the start of each frame).
    pub fn begin_frame(&mut self) {
        self.locates.clear();
    }

    /// The document bytes, or `None` when there is no document.
    pub fn document(&self) -> Option<&[u8]> {
        self.document.as_deref()
    }

    pub fn region(&self) -> Region {
        self.region
    }

    /// The scroll value as stored (a signed number; see [`Page::scroll_for`]).
    pub fn scroll(&self) -> u32 {
        self.scroll
    }

    /// The scroll value the host presents for a document `height` units
    /// high: the stored value as a signed number, clamped to
    /// 0 … max(0, height − h).
    pub fn scroll_for(&self, height: u32) -> u32 {
        let max = height.saturating_sub(self.region.h as u32) as i64;
        (self.scroll as i32 as i64).clamp(0, max) as u32
    }

    /// The background as (r, g, b, a).
    pub fn background(&self) -> [u8; 4] {
        self.background.to_le_bytes()
    }

    /// Changes at every accepted submit and every clear.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The page section as `write_state` wrote it (STATE.md 4). A restored
    /// document counts as new: a host lays it out and reports its height.
    pub(crate) fn restore(&mut self, r: &mut crate::snapshot::Reader) -> Result<(), crate::snapshot::RestoreError> {
        let (x, y, w, h) = (r.u16()?, r.u16()?, r.u16()?, r.u16()?);
        self.set_region(x as u32, y as u32, w as u32, h as u32);
        self.scroll = r.u32()?;
        self.background = r.u32()?;
        let d = r.u32()? as usize;
        if d > MAX_DOCUMENT {
            return Err(crate::snapshot::bad("document length"));
        }
        self.document = (d > 0).then(|| r.take(d).map(<[u8]>::to_vec)).transpose()?;
        self.generation = self.document.is_some() as u64;
        Ok(())
    }

    /// The rectangle, scroll value, background and document (STATE.md,
    /// page section); not the generation counter.
    pub(crate) fn write_state(&self, out: &mut Vec<u8>) {
        let r = self.region;
        for v in [r.x, r.y, r.w, r.h] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.scroll.to_le_bytes());
        out.extend_from_slice(&self.background.to_le_bytes());
        // No document is length 0 (a document is at least one byte).
        let doc = self.document.as_deref().unwrap_or(&[]);
        out.extend_from_slice(&(doc.len() as u32).to_le_bytes());
        out.extend_from_slice(doc);
    }
}

/// Alignment of a paragraph, relative to its text direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Start,
    Centre,
    End,
}

/// The text state a run was added with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Style {
    /// [`face::SANS`], [`face::SERIF`] or [`face::MONO`].
    pub face: u8,
    pub bold: bool,
    pub italic: bool,
    /// Screen units, 6 to 96.
    pub size: u8,
    pub rgb: [u8; 3],
    /// 0: no anchor.
    pub anchor: u16,
}

impl Default for Style {
    fn default() -> Self {
        Style { face: face::SANS, bold: false, italic: false, size: DEFAULT_SIZE, rgb: DEFAULT_COLOUR, anchor: 0 }
    }
}

/// Text in one style. The text is already cleaned (SPEC 11.3): no control
/// characters but U+000A (a line break); tabs are spaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub style: Style,
}

/// A paragraph: text, or a rule.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paragraph {
    pub align: Align,
    /// Screen units from the start edge, for every line.
    pub indent: u8,
    /// Screen units of gap before the paragraph.
    pub space: u8,
    /// Lines break only at U+000A and are clipped at the rectangle's edge.
    pub nowrap: bool,
    /// With `nowrap`: a line that does not fit ends in an ellipsis.
    pub ellipsis: bool,
    pub runs: Vec<Run>,
    /// A rule paragraph: a line in this colour across the rectangle.
    pub rule: Option<[u8; 3]>,
    /// A figure paragraph (then `align` is the figure's alignment).
    pub figure: Option<Figure>,
    /// A table paragraph.
    pub table: Option<Table>,
}

/// A table as the document gives it (SPEC 11.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    /// 1 to 8.
    pub columns: u8,
    /// The first row is a header: its text is bold (already applied to the
    /// runs) and a rule is drawn under it.
    pub header: bool,
    /// Each column's alignment.
    pub align: Vec<Align>,
    /// The colour of the rules: the text colour at the Table command.
    pub rgb: [u8; 3],
    /// The cells, row after row; the last row may be short (its missing
    /// cells are empty). Each cell is its runs, without empty ones.
    pub cells: Vec<Vec<Run>>,
}

impl Table {
    /// The number of rows.
    pub fn rows(&self) -> usize {
        self.cells.len().div_ceil(self.columns as usize)
    }

    /// The cell at (row, column); empty if the last row has no such cell.
    pub fn cell(&self, row: usize, column: usize) -> &[Run] {
        self.cells.get(row * self.columns as usize + column).map_or(&[], Vec::as_slice)
    }

    /// The columns' widths from their natural widths (SPEC 11.4 step 3):
    /// unchanged if they fit in `width`, else each narrowed to a common cap,
    /// the largest that fits but at least `floor`. Any unit (the reference
    /// layout uses half units).
    pub fn column_widths(natural: &[u32], width: u32, floor: u32) -> Vec<u32> {
        let total: u64 = natural.iter().map(|&w| w as u64).sum();
        if total <= width as u64 {
            return natural.to_vec();
        }
        let fits = |cap: u32| natural.iter().map(|&w| w.min(cap) as u64).sum::<u64>() <= width as u64;
        // The largest cap that fits: the sum grows with the cap, and the
        // whole natural width does not fit.
        let (mut lo, mut hi) = (0u32, natural.iter().copied().max().unwrap_or(0));
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            if fits(mid) { lo = mid } else { hi = mid - 1 }
        }
        let cap = lo.max(floor);
        natural.iter().map(|&w| w.min(cap)).collect()
    }
}

impl Paragraph {
    /// Whether the paragraph has any characters (a rule or figure has none).
    pub fn has_text(&self) -> bool {
        self.runs.iter().any(|r| !r.text.is_empty())
    }

    /// Whether it is a text paragraph (not a rule, a figure or a table).
    fn is_text(&self) -> bool {
        self.rule.is_none() && self.figure.is_none() && self.table.is_none()
    }
}

/// Figure sizes are clamped to this range of screen units (SPEC 11.3).
pub const MAX_FIGURE_SIDE: u16 = 1024;
/// The per-figure cap on painted area in square screen units (SPEC 11.3).
pub const FIGURE_AREA_CAP: u64 = 4 * 640 * 480;
/// The per-figure cap on path length in screen units (SPEC 11.3).
pub const FIGURE_LENGTH_CAP: u64 = 65_536;
/// The most paths a figure draws (SPEC 11.3).
pub const FIGURE_PATH_CAP: usize = 256;
/// Initial stroke width in sixteenths (one screen unit).
pub const DEFAULT_STROKE_WIDTH: u16 = 16;

/// A figure as the document gives it (SPEC 11.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Figure {
    /// Screen units, 1 to 1024.
    pub width: u16,
    pub height: u16,
    /// The text colour when the figure appeared: the initial stroke colour.
    pub rgb: [u8; 3],
    /// 0: no anchor.
    pub anchor: u16,
    /// The path commands, as given.
    pub path: Vec<u8>,
}

/// One segment of a figure path; points in sixteenths of a screen unit from
/// the figure's top-left corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Segment {
    Move(i16, i16),
    Line(i16, i16),
    Quad(i16, i16, i16, i16),
    Cubic(i16, i16, i16, i16, i16, i16),
    Close,
}

/// A path to draw: its subpaths, filled (non-zero) then stroked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FigurePath {
    /// Fill colour (r, g, b, a), if filled.
    pub fill: Option<[u8; 4]>,
    /// Stroke colour and width in sixteenths, if stroked.
    pub stroke: Option<([u8; 4], u16)>,
    /// Every subpath starts with a Move.
    pub segments: Vec<Segment>,
}

impl FigurePath {
    /// The area this path counts against the cap (SPEC 11.3), in square
    /// screen units: the box of its points, clipped to the figure's box,
    /// for its fill, and that box grown by half the stroke width, clipped,
    /// for its stroke; each rounded up.
    fn area(&self, width: u16, height: u16) -> u64 {
        let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
        for s in &self.segments {
            let pts: &[(i16, i16)] = &match *s {
                Segment::Move(x, y) | Segment::Line(x, y) => vec![(x, y)],
                Segment::Quad(a, b, x, y) => vec![(a, b), (x, y)],
                Segment::Cubic(a, b, c, d, x, y) => vec![(a, b), (c, d), (x, y)],
                Segment::Close => vec![],
            };
            for &(x, y) in pts {
                x0 = x0.min(x as i64);
                y0 = y0.min(y as i64);
                x1 = x1.max(x as i64);
                y1 = y1.max(y as i64);
            }
        }
        if x0 > x1 {
            return 0;
        }
        let (w, h) = (16 * width as i64, 16 * height as i64);
        let clipped = |grow: i64| -> u64 {
            let cw = (x1 + grow).min(w) - (x0 - grow).max(0);
            let ch = (y1 + grow).min(h) - (y0 - grow).max(0);
            if cw <= 0 || ch <= 0 { 0 } else { (cw as u64 * ch as u64).div_ceil(256) }
        };
        let mut area = 0;
        if self.fill.is_some() {
            area += clipped(0);
        }
        if let Some((_, sw)) = self.stroke {
            area += clipped((sw as i64 + 1) / 2);
        }
        area
    }
}

impl FigurePath {
    /// The length this path counts against the cap (SPEC 11.3), in screen
    /// units, rounded up: for each Line, Quadratic and Cubic, |dx| + |dy|
    /// over the steps from its start through its control points to its end,
    /// summed; then for the fill and for the stroke each, twice that plus
    /// four times the width (for the fill, one unit) for each command.
    fn length(&self) -> u64 {
        let (mut len, mut n) = (0u64, 0u64);
        let mut at = (0i64, 0i64);
        fn step(to: (i16, i16), at: &mut (i64, i64), len: &mut u64) {
            let to = (to.0 as i64, to.1 as i64);
            *len += (to.0 - at.0).unsigned_abs() + (to.1 - at.1).unsigned_abs();
            *at = to;
        }
        let mut start = (0i16, 0i16);
        for s in &self.segments {
            match *s {
                Segment::Move(x, y) => {
                    at = (x as i64, y as i64);
                    start = (x, y);
                }
                Segment::Line(x, y) => {
                    step((x, y), &mut at, &mut len);
                    n += 1;
                }
                Segment::Quad(a, b, x, y) => {
                    step((a, b), &mut at, &mut len);
                    step((x, y), &mut at, &mut len);
                    n += 1;
                }
                Segment::Cubic(a, b, c, d, x, y) => {
                    step((a, b), &mut at, &mut len);
                    step((c, d), &mut at, &mut len);
                    step((x, y), &mut at, &mut len);
                    n += 1;
                }
                Segment::Close => at = (start.0 as i64, start.1 as i64),
            }
        }
        let mut total = 0;
        if self.fill.is_some() {
            total += 2 * len + 4 * DEFAULT_STROKE_WIDTH as u64 * n;
        }
        if let Some((_, w)) = self.stroke {
            total += 2 * len + 4 * w as u64 * n;
        }
        total.div_ceil(16)
    }
}

impl Figure {
    /// The figure's area cap in square screen units: 8 × its area, at most
    /// [`FIGURE_AREA_CAP`].
    pub fn area_cap(&self) -> u64 {
        (8 * self.width as u64 * self.height as u64).min(FIGURE_AREA_CAP)
    }

    /// Decodes the path commands (SPEC 11.3) into the paths to draw, in
    /// order: up to an unknown opcode or cut-off operands, without paths
    /// that draw nothing, and only as far as the caps allow (at most
    /// [`FIGURE_PATH_CAP`] paths, area and length).
    pub fn paths(&self) -> Vec<FigurePath> {
        let b = &self.path;
        let i16_at = |i: usize| i16::from_le_bytes([b[i], b[i + 1]]);
        let mut out = Vec::new();
        let mut fill: Option<[u8; 4]> = None;
        let mut stroke: Option<([u8; 4], u16)> = Some(([self.rgb[0], self.rgb[1], self.rgb[2], 255], DEFAULT_STROKE_WIDTH));
        let mut segments: Vec<Segment> = Vec::new();
        // Current point, start of the open subpath (None: no open subpath).
        let (mut cx, mut cy) = (0i16, 0i16);
        let mut start: Option<(i16, i16)> = None;
        let (mut total, mut total_len) = (0u64, 0u64);
        let cap = self.area_cap();
        let mut capped = false;
        let mut flush = |segments: &mut Vec<Segment>, fill: Option<[u8; 4]>, stroke: Option<([u8; 4], u16)>| {
            let draws = segments.iter().any(|s| !matches!(s, Segment::Move(..)));
            if draws && !capped && (fill.is_some() || stroke.is_some()) {
                let p = FigurePath { fill, stroke, segments: std::mem::take(segments) };
                let area = p.area(self.width, self.height);
                let len = p.length();
                if total + area > cap || total_len + len > FIGURE_LENGTH_CAP || out.len() == FIGURE_PATH_CAP {
                    capped = true;
                } else {
                    total += area;
                    total_len += len;
                    out.push(p);
                }
            }
            segments.clear();
        };
        let mut at = 0;
        while at < b.len() {
            let n = match b[at] {
                path_op::FILL => 4,
                path_op::STROKE => 6,
                path_op::MOVE | path_op::LINE => 4,
                path_op::QUAD => 8,
                path_op::CUBIC => 12,
                path_op::CLOSE => 0,
                _ => break,
            };
            if at + 1 + n > b.len() {
                break;
            }
            let a = at + 1;
            let opcode = b[at];
            if matches!(opcode, path_op::LINE | path_op::QUAD | path_op::CUBIC) && start.is_none() {
                segments.push(Segment::Move(cx, cy));
                start = Some((cx, cy));
            }
            match opcode {
                path_op::FILL | path_op::STROKE => {
                    flush(&mut segments, fill, stroke);
                    start = None;
                    let rgba = [b[a], b[a + 1], b[a + 2], b[a + 3]];
                    if opcode == path_op::FILL {
                        fill = (rgba[3] != 0).then_some(rgba);
                    } else {
                        let w = u16::from_le_bytes([b[a + 4], b[a + 5]]);
                        stroke = (rgba[3] != 0 && w != 0).then_some((rgba, w));
                    }
                }
                path_op::MOVE => {
                    (cx, cy) = (i16_at(a), i16_at(a + 2));
                    segments.push(Segment::Move(cx, cy));
                    start = Some((cx, cy));
                }
                path_op::LINE => {
                    (cx, cy) = (i16_at(a), i16_at(a + 2));
                    segments.push(Segment::Line(cx, cy));
                }
                path_op::QUAD => {
                    (cx, cy) = (i16_at(a + 4), i16_at(a + 6));
                    segments.push(Segment::Quad(i16_at(a), i16_at(a + 2), cx, cy));
                }
                path_op::CUBIC => {
                    (cx, cy) = (i16_at(a + 8), i16_at(a + 10));
                    segments.push(Segment::Cubic(i16_at(a), i16_at(a + 2), i16_at(a + 4), i16_at(a + 6), cx, cy));
                }
                _ => {
                    if let Some(s) = start.take() {
                        segments.push(Segment::Close);
                        (cx, cy) = s;
                    }
                }
            }
            at += 1 + n;
        }
        flush(&mut segments, fill, stroke);
        out
    }
}

/// Decodes text for a Text command: malformed UTF-8 as U+FFFD (the maximal
/// subpart rule, as the terminal does), tab as space, other control
/// characters (U+0000-U+001F but U+000A, U+007F-U+009F) dropped.
fn clean(bytes: &[u8], out: &mut String) {
    for ch in String::from_utf8_lossy(bytes).chars() {
        match ch {
            '\n' => out.push('\n'),
            '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
}

/// Decodes a document (SPEC 11.3) into paragraphs. It ends at End, at the
/// end of the bytes, at an unknown opcode, at a command whose operands are
/// cut off or at a Table whose column count is not 1 to 8; the commands
/// before that are used. There is always at least the implicit first
/// paragraph.
pub fn parse(doc: &[u8]) -> Vec<Paragraph> {
    let mut paragraphs = vec![Paragraph::default()];
    // Properties of the latest Paragraph command, for text after a rule.
    let mut props = Paragraph::default();
    let mut style = Style::default();
    // The table open (the last paragraph), whether it takes more cells, and
    // the cells of all tables so far.
    let mut table_open = false;
    let mut taking = false;
    let mut doc_cells = 0usize;
    let mut at = 0;
    while at < doc.len() {
        let operands = match doc[at] {
            op::END => break,
            op::TEXT => match doc.get(at + 1..at + 3) {
                Some(n) => 2 + u16::from_le_bytes([n[0], n[1]]) as usize,
                None => break,
            },
            op::FONT => 2,
            op::SIZE => 1,
            op::COLOUR => 3,
            op::PARAGRAPH => 4,
            op::RULE => 0,
            op::ANCHOR => 2,
            op::FIGURE => match doc.get(at + 6..at + 8) {
                Some(n) => 7 + u16::from_le_bytes([n[0], n[1]]) as usize,
                None => break,
            },
            op::TABLE => match doc.get(at + 1) {
                Some(&n) if (1..=MAX_TABLE_COLUMNS).contains(&n) => 2 + n as usize,
                _ => break,
            },
            op::CELL | op::END_TABLE => 0,
            _ => break,
        };
        let Some(args) = doc.get(at + 1..at + 1 + operands) else { break };
        if table_open && matches!(doc[at], op::PARAGRAPH | op::RULE | op::FIGURE | op::TABLE | op::END_TABLE) {
            table_open = false;
        }
        match doc[at] {
            op::TEXT if table_open => {
                // Into the current cell; before the first Cell, or after the
                // limits, the text is ignored.
                let t = paragraphs.last_mut().unwrap().table.as_mut().unwrap();
                if let Some(cell) = t.cells.last_mut().filter(|_| taking) {
                    match cell.last_mut() {
                        Some(run) if run.style == style => clean(&args[2..], &mut run.text),
                        _ => {
                            let mut text = String::new();
                            clean(&args[2..], &mut text);
                            cell.push(Run { text, style });
                        }
                    }
                }
            }
            op::TEXT => {
                let last = paragraphs.last_mut().unwrap();
                if !last.is_text() {
                    // Text after a rule, figure or table starts a paragraph
                    // like the latest one.
                    paragraphs.push(props.clone());
                }
                let p = paragraphs.last_mut().unwrap();
                match p.runs.last_mut() {
                    Some(run) if run.style == style => clean(&args[2..], &mut run.text),
                    _ => {
                        let mut text = String::new();
                        clean(&args[2..], &mut text);
                        p.runs.push(Run { text, style });
                    }
                }
            }
            op::FONT => {
                style.face = if args[0] <= face::MONO { args[0] } else { face::SANS };
                style.bold = args[1] & 1 != 0;
                style.italic = args[1] & 2 != 0;
            }
            op::SIZE => style.size = args[0].clamp(MIN_SIZE, MAX_SIZE),
            op::COLOUR => style.rgb = [args[0], args[1], args[2]],
            op::PARAGRAPH => {
                props = Paragraph {
                    align: align_of(args[0]),
                    indent: args[1],
                    space: args[2],
                    nowrap: args[3] & 1 != 0,
                    ellipsis: args[3] & 3 == 3,
                    runs: Vec::new(),
                    rule: None,
                    figure: None,
                    table: None,
                };
                paragraphs.push(props.clone());
            }
            op::RULE => paragraphs.push(Paragraph { rule: Some(style.rgb), ..Paragraph::default() }),
            op::ANCHOR => style.anchor = u16::from_le_bytes([args[0], args[1]]),
            op::FIGURE => {
                let side = |i: usize| u16::from_le_bytes([args[i], args[i + 1]]).clamp(1, MAX_FIGURE_SIDE);
                let figure = Figure {
                    width: side(0),
                    height: side(2),
                    rgb: style.rgb,
                    anchor: style.anchor,
                    path: args[7..].to_vec(),
                };
                let space = block_space(&mut paragraphs, &props);
                paragraphs.push(Paragraph { align: align_of(args[4]), space, figure: Some(figure), ..Paragraph::default() });
            }
            op::TABLE => {
                let table = Table {
                    columns: args[0],
                    header: args[1] & 1 != 0,
                    align: args[2..].iter().map(|&a| align_of(a)).collect(),
                    rgb: style.rgb,
                    cells: Vec::new(),
                };
                let space = block_space(&mut paragraphs, &props);
                paragraphs.push(Paragraph { space, table: Some(table), ..Paragraph::default() });
                table_open = true;
                taking = true;
            }
            op::CELL if table_open && taking => {
                let t = paragraphs.last_mut().unwrap().table.as_mut().unwrap();
                if t.cells.len() < MAX_TABLE_CELLS && doc_cells < MAX_DOCUMENT_CELLS {
                    t.cells.push(Vec::new());
                    doc_cells += 1;
                } else {
                    taking = false;
                }
            }
            op::CELL | op::END_TABLE => {}
            _ => unreachable!(),
        }
        at += 1 + operands;
    }
    for p in &mut paragraphs {
        p.runs.retain(|r| !r.text.is_empty());
        if let Some(t) = &mut p.table {
            let header = if t.header { t.columns as usize } else { 0 };
            for (i, cell) in t.cells.iter_mut().enumerate() {
                cell.retain(|r| !r.text.is_empty());
                if i < header {
                    for run in cell.iter_mut() {
                        run.style.bold = true;
                    }
                }
            }
        }
    }
    paragraphs
}

fn align_of(b: u8) -> Align {
    match b {
        1 => Align::Centre,
        2 => Align::End,
        _ => Align::Start,
    }
}

/// The space above a figure or table, which takes the place of the
/// current paragraph if that has no text yet (and its space), else follows
/// it with the space of the latest Paragraph command.
fn block_space(paragraphs: &mut Vec<Paragraph>, props: &Paragraph) -> u8 {
    let last = paragraphs.last().unwrap();
    if last.is_text() && !last.has_text() {
        let space = last.space;
        paragraphs.pop();
        space
    } else {
        props.space
    }
}

/// A span of one style on a laid-out line: its x range in half screen units
/// from the rectangle's left edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub x0: u32,
    pub x1: u32,
    pub text: String,
    pub style: Style,
}

/// One line of the reference layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// Top of the line, in screen units from the top of the document.
    pub y: u32,
    pub height: u32,
    /// The spans, left to right (empty for an empty line and a rule).
    pub spans: Vec<Span>,
    /// A rule: a line across the rectangle in this colour, at its middle.
    pub rule: Option<[u8; 3]>,
    /// Lines of a no-wrap paragraph may be wider than the rectangle.
    pub nowrap: bool,
    /// A figure: its box on the line (the line is as high as the figure).
    pub figure: Option<FigureBox>,
    /// A table's rule (then `rule` is its colour and the height is 0): it
    /// spans only the table's width.
    pub rule_span: Option<RuleSpan>,
}

/// Where a table's rule is drawn: x from `x0` to `x1` in half screen units
/// from the rectangle's left edge, at the line's y; faint between rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuleSpan {
    pub x0: u32,
    pub x1: u32,
    pub faint: bool,
}

/// Where a figure was laid out: x in screen units from the rectangle's left
/// edge, the size given and the width shown (clipped to the rectangle).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FigureBox {
    pub x: u32,
    pub width: u16,
    pub height: u16,
    pub shown: u32,
    pub anchor: u16,
}

impl FigureBox {
    /// The box of a figure aligned in a rectangle `rect` units wide: start
    /// is left, centre puts half the leftover width (rounded down) before
    /// it, end all of it; a figure wider than the rectangle starts at its
    /// left edge and is cut at its right.
    pub fn place(figure: &Figure, align: Align, rect: u32) -> FigureBox {
        let left = rect.saturating_sub(figure.width as u32);
        let x = match align {
            Align::Start => 0,
            Align::Centre => left / 2,
            Align::End => left,
        };
        FigureBox { x, width: figure.width, height: figure.height, shown: (figure.width as u32).min(rect - x), anchor: figure.anchor }
    }
}

/// The reference layout of a document at a width (SPEC 11.6).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// The document's height in screen units.
    pub height: u32,
    pub lines: Vec<Line>,
}

/// Advance of `ch` in half screen units at `size`: 0.5 × size, or 1 × size
/// for a wide character (the terminal's table; every character above
/// U+FFFF counts as wide).
fn advance(ch: char, size: u8) -> u32 {
    let wide = ch as u32 > 0xFFFF || is_wide(ch as u16);
    size as u32 * if wide { 2 } else { 1 }
}

/// Line height for a size: ceil(1.25 × size).
pub fn line_height(size: u8) -> u32 {
    (5 * size as u32).div_ceil(4)
}

/// The reference layout (SPEC 11.6): every character advances half its size
/// (a wide one its whole size), lines are ceil(1.25 × size) high (the
/// largest size on the line), text wraps greedily at spaces and a word
/// longer than the line is split; rules are 8 units high. Text is laid out
/// left to right whatever its script. Tables as SPEC 11.4 says.
pub fn reference_layout(doc: &[u8], width: u16) -> Layout {
    let mut layout = Layout::default();
    let full = 2 * width as u32;
    let mut y: u32 = 0;
    for p in parse(doc) {
        y = y.saturating_add(p.space as u32);
        if let Some(rgb) = p.rule {
            layout.lines.push(Line { y, height: RULE_HEIGHT, spans: Vec::new(), rule: Some(rgb), nowrap: false, figure: None, rule_span: None });
            y = y.saturating_add(RULE_HEIGHT);
            continue;
        }
        if let Some(f) = &p.figure {
            let b = FigureBox::place(f, p.align, width as u32);
            let height = f.height as u32;
            layout.lines.push(Line { y, height, spans: Vec::new(), rule: None, nowrap: false, figure: Some(b), rule_span: None });
            y = y.saturating_add(height);
            continue;
        }
        if let Some(t) = &p.table {
            y = reference_table(t, full, y, &mut layout.lines);
            continue;
        }
        if !p.has_text() {
            continue;
        }
        let avail = full.saturating_sub(2 * p.indent as u32);
        y = reference_text(&p.runs, avail, 2 * p.indent as u32, p.align, (p.nowrap, p.ellipsis), y, &mut layout.lines);
    }
    layout.height = y;
    layout
}

/// The width of a cell's text broken only at line breaks: its widest line,
/// in half units.
fn natural_width(runs: &[Run]) -> u32 {
    let (mut widest, mut line) = (0, 0);
    for run in runs {
        for ch in run.text.chars() {
            if ch == '\n' {
                line = 0;
            } else {
                line += advance(ch, run.style.size);
                widest = widest.max(line);
            }
        }
    }
    widest
}

/// A table in the reference layout (SPEC 11.4, 11.6), with its top at `y`
/// in a rectangle `full` half units wide; returns its bottom.
fn reference_table(t: &Table, full: u32, y: u32, lines: &mut Vec<Line>) -> u32 {
    let n = t.columns as usize;
    let (pad_x, pad_y) = (2 * CELL_PAD_X, CELL_PAD_Y);
    let mut natural = vec![2 * pad_x; n];
    for (i, cell) in t.cells.iter().enumerate() {
        let w = &mut natural[i % n];
        *w = (*w).max(natural_width(cell) + 2 * pad_x);
    }
    let widths = Table::column_widths(&natural, full, 2 * MIN_COLUMN_CAP);
    let table_w: u32 = widths.iter().sum();
    let mut top = y;
    let rows = t.rows();
    for row in 0..rows {
        let mut row_lines = Vec::new();
        let mut tallest = 0;
        let mut x = 0;
        for (c, &w) in widths.iter().enumerate() {
            let runs = t.cell(row, c);
            if !runs.is_empty() {
                let align = t.align.get(c).copied().unwrap_or_default();
                let bottom = reference_text(runs, w - 2 * pad_x, x + pad_x, align, (false, false), top + pad_y, &mut row_lines);
                tallest = tallest.max(bottom - (top + pad_y));
            }
            x += w;
        }
        // The lines of a row in order of their tops, cells left to right.
        row_lines.sort_by_key(|l: &Line| l.y);
        lines.append(&mut row_lines);
        top = top.saturating_add(tallest + 2 * pad_y);
        let header_rule = t.header && row == 0;
        if header_rule || row + 1 < rows {
            let span = RuleSpan { x0: 0, x1: table_w, faint: !header_rule };
            lines.push(Line { y: top, height: 0, spans: Vec::new(), rule: Some(t.rgb), nowrap: false, figure: None, rule_span: Some(span) });
        }
    }
    top
}

/// Lays out `runs` as the lines of a paragraph `avail` half units wide
/// starting `x0` half units from the rectangle's left edge, the first at
/// `y`; returns the bottom of the last.
fn reference_text(runs: &[Run], avail: u32, x0: u32, align: Align, (nowrap, ellipsis): (bool, bool), mut y: u32, out: &mut Vec<Line>) -> u32 {
    // Split into segments at line breaks; each segment remembers the
    // style in effect at its end, for the height of an empty line.
    let mut segments: Vec<(Vec<(char, Style)>, Style)> = vec![(Vec::new(), runs[0].style)];
    for run in runs {
        for ch in run.text.chars() {
            let seg = segments.last_mut().unwrap();
            if ch == '\n' {
                seg.1 = run.style;
                segments.push((Vec::new(), run.style));
            } else {
                seg.0.push((ch, run.style));
                seg.1 = run.style;
            }
        }
    }
    for (chars, end_style) in segments {
        let mut lines: Vec<Vec<(char, Style)>> = Vec::new();
        if nowrap || chars.is_empty() {
            lines.push(chars);
        } else {
            wrap(&chars, avail, &mut lines);
        }
        for mut line in lines {
            if ellipsis {
                ellipsize(&mut line, avail);
            }
            let height = line.iter().map(|(_, s)| line_height(s.size)).max().unwrap_or(line_height(end_style.size));
            let w: u32 = line.iter().map(|&(c, s)| advance(c, s.size)).sum();
            let offset = match align {
                Align::Start => 0,
                Align::Centre => avail.saturating_sub(w) / 2,
                Align::End => avail.saturating_sub(w),
            };
            let mut x = x0 + offset;
            let mut spans: Vec<Span> = Vec::new();
            for (c, s) in line {
                let a = advance(c, s.size);
                match spans.last_mut() {
                    Some(span) if span.style == s => {
                        span.text.push(c);
                        span.x1 += a;
                    }
                    _ => spans.push(Span { x0: x, x1: x + a, text: c.to_string(), style: s }),
                }
                x += a;
            }
            out.push(Line { y, height, spans, rule: None, nowrap, figure: None, rule_span: None });
            y = y.saturating_add(height);
        }
    }
    y
}

/// A no-wrap line with an ellipsis (SPEC 11.6): if it is wider than
/// `avail` half units, characters are dropped from its end until the rest
/// and an ellipsis in the style of the last one dropped fit (or none is
/// left), and the ellipsis is added.
fn ellipsize(line: &mut Vec<(char, Style)>, avail: u32) {
    let mut width: u32 = line.iter().map(|&(c, s)| advance(c, s.size)).sum();
    if width <= avail {
        return;
    }
    let mut style = line[line.len() - 1].1;
    while let Some((c, s)) = line.pop() {
        width -= advance(c, s.size);
        style = s;
        if width + advance('\u{2026}', s.size) <= avail {
            break;
        }
    }
    line.push(('\u{2026}', style));
}

/// Greedy wrapping at spaces within `avail` half units. Spaces where a line
/// breaks are dropped; a word wider than the line is split, at least one
/// character per line.
fn wrap(chars: &[(char, Style)], avail: u32, lines: &mut Vec<Vec<(char, Style)>>) {
    let adv = |&(c, s): &(char, Style)| advance(c, s.size);
    let mut line: Vec<(char, Style)> = Vec::new();
    let mut width = 0;
    let mut i = 0;
    let mut first = true;
    while i < chars.len() {
        // Spaces before the next word.
        let space_start = i;
        while i < chars.len() && chars[i].0 == ' ' {
            i += 1;
        }
        let spaces = &chars[space_start..i];
        let word_start = i;
        while i < chars.len() && chars[i].0 != ' ' {
            i += 1;
        }
        let word = &chars[word_start..i];
        let space_w: u32 = spaces.iter().map(adv).sum();
        let word_w: u32 = word.iter().map(adv).sum();
        if first || width + space_w + word_w <= avail {
            // Leading spaces of the segment are kept; so are spaces that fit.
            line.extend_from_slice(spaces);
            width += space_w;
        } else {
            lines.push(std::mem::take(&mut line));
            width = 0;
        }
        first = false;
        if width + word_w <= avail {
            line.extend_from_slice(word);
            width += word_w;
            continue;
        }
        // Split the word.
        for c in word {
            let a = adv(c);
            if width + a > avail && !line.iter().all(|(c, _)| *c == ' ') {
                lines.push(std::mem::take(&mut line));
                width = 0;
            }
            line.push(*c);
            width += a;
        }
    }
    // Trailing spaces do not take room.
    lines.push(line);
}

impl Layout {
    /// The lines that intersect the rectangle when scrolled to `scroll`
    /// (already clamped) with a rectangle `h` units high.
    pub fn visible(&self, scroll: u32, h: u16) -> impl Iterator<Item = &Line> {
        let bottom = scroll.saturating_add(h as u32);
        self.lines.iter().filter(move |l| l.y < bottom && l.y.saturating_add(l.height.max(1)) > scroll)
    }

    /// Where anchor `id` is (SPEC 11.5): the top and height of the first
    /// line or figure that carries it; `None` if none does.
    pub fn locate(&self, id: u16) -> Option<(u32, u32)> {
        self.lines
            .iter()
            .find(|l| l.figure.is_some_and(|f| f.anchor == id) || l.spans.iter().any(|s| s.style.anchor == id))
            .map(|l| (l.y, l.height))
    }

    /// The anchor of the text at (x, y), in screen units from the rectangle's
    /// left edge and the document's top; 0 if none.
    pub fn anchor_at(&self, x: u32, y: u32) -> u16 {
        let hx = 2 * x;
        for l in self.lines.iter().filter(|l| l.y <= y && y < l.y + l.height) {
            if let Some(f) = l.figure {
                if f.x <= x && x < f.x + f.shown {
                    return f.anchor;
                }
            }
            if let Some(s) = l.spans.iter().find(|s| s.x0 <= hx && hx < s.x1) {
                return s.style.anchor;
            }
        }
        0
    }

    /// The visible lines as text (`terse run --dump-page`): a header with the
    /// height and scroll, then one line per visible line:
    /// `<y> <height> <x>  <face><size>[b][i] |<text>`, with anchored text as
    /// `[text]{id}`, a rule as `-- rule --`, a figure as `-- figure WxH --`
    /// (then `{id}` if anchored), and x in screen units.
    pub fn dump(&self, page: &Page) -> String {
        let r = page.region();
        let scroll = page.scroll_for(self.height);
        let mut out = format!(
            "-- page: height {}, scroll {} (stored {}), region {} {} {} {} --\n",
            self.height,
            scroll,
            page.scroll() as i32,
            r.x,
            r.y,
            r.w,
            r.h
        );
        if !r.visible() {
            return out;
        }
        for line in self.visible(scroll, r.h) {
            let _ = write!(out, "{:5} {:3} ", line.y, line.height);
            if let Some(r) = line.rule_span {
                let kind = if r.faint { "faint rule" } else { "rule" };
                let _ = writeln!(out, "{:5}  -- {kind} to {} --", r.x0 / 2, r.x1.div_ceil(2));
                continue;
            }
            if line.rule.is_some() {
                out.push_str("    0  -- rule --\n");
                continue;
            }
            if let Some(f) = line.figure {
                let _ = write!(out, "{:5}  -- figure {}x{} --", f.x, f.width, f.height);
                if f.anchor != 0 {
                    let _ = write!(out, " {{{}}}", f.anchor);
                }
                out.push('\n');
                continue;
            }
            let Some(first) = line.spans.first() else {
                out.push_str("    0\n");
                continue;
            };
            let s = first.style;
            let face = ["sans", "serif", "mono"][s.face as usize];
            let _ = write!(
                out,
                "{:5}{} {face}{}{}{} |",
                first.x0 / 2,
                if first.x0 % 2 == 1 { ".5" } else { "  " },
                s.size,
                if s.bold { "b" } else { "" },
                if s.italic { "i" } else { "" }
            );
            for span in &line.spans {
                if span.style.anchor != 0 {
                    let _ = write!(out, "[{}]{{{}}}", span.text, span.style.anchor);
                } else {
                    out.push_str(&span.text);
                }
            }
            out.push('\n');
        }
        out
    }
}

/// When a host lays the page out and reports its height (SPEC 11.5): at
/// every new document (submit or clear) and when the rectangle's width
/// changes the height. Host bookkeeping, not machine state.
#[derive(Clone, Debug, Default)]
pub struct PageWatch {
    /// The (generation, width) laid out last.
    done: Option<(u64, u16)>,
    /// The generation and height reported last.
    reported: Option<(u64, u32)>,
}

/// What to lay out: a document generation at a width.
pub type LayoutKey = (u64, u16);

impl PageWatch {
    /// The layout the host should make now, if the page has changed since
    /// the last one. A document is laid out only once the rectangle has a
    /// width; no document has height 0 at any width.
    pub fn wanted(&self, page: &Page) -> Option<LayoutKey> {
        let width = if page.document().is_some() { page.region().w } else { 0 };
        let key = (page.generation(), width);
        if self.done == Some(key) || (page.document().is_some() && width == 0) {
            return None;
        }
        // The very first state (no document ever) needs no report.
        if self.done.is_none() && page.generation() == 0 {
            return None;
        }
        Some(key)
    }

    /// A layout for `key` finished with `height`: returns the event to send
    /// at the start of the next frame, if any (a new document, or a new
    /// height for the same one).
    pub fn finished(&mut self, key: LayoutKey, height: u32) -> Option<Event> {
        self.done = Some(key);
        let height = height.min(MAX_REPORTED_HEIGHT);
        if self.reported.is_some_and(|(g, h)| g == key.0 && h == height) {
            return None;
        }
        self.reported = Some((key.0, height));
        Some(Event::page_laid_out(height))
    }
}

/// Something that lays a document out for a host: the reference layout, or
/// a host's fonts.
pub trait Layouter {
    /// Lays `doc` out at `width` screen units and returns its height; the
    /// result is kept for [`Layouter::anchor_at`].
    fn layout(&mut self, doc: &[u8], width: u16) -> u32;
    /// The anchor of the text at (x, y) in the last layout, in screen units
    /// from the rectangle's left edge and the document's top; 0 if none.
    fn anchor_at(&self, x: u32, y: u32) -> u16;
    /// The height of the last layout.
    fn height(&self) -> u32;
    /// Where anchor `id` is in the last layout: the top of the first line
    /// or figure that carries it and its height, in whole screen units (the
    /// top rounded down, the bottom up); `None` if none does.
    fn locate(&self, id: u16) -> Option<(u32, u32)>;
}

/// The reference layout as a [`Layouter`].
#[derive(Clone, Debug, Default)]
pub struct ReferenceLayouter {
    pub layout: Layout,
}

impl Layouter for ReferenceLayouter {
    fn layout(&mut self, doc: &[u8], width: u16) -> u32 {
        self.layout = reference_layout(doc, width);
        self.layout.height
    }

    fn anchor_at(&self, x: u32, y: u32) -> u16 {
        self.layout.anchor_at(x, y)
    }

    fn height(&self) -> u32 {
        self.layout.height
    }

    fn locate(&self, id: u16) -> Option<(u32, u32)> {
        self.layout.locate(id)
    }
}

/// The page events of a host that lays out synchronously between frames
/// (`terse run`, `terse-host --screenshot`): the height one frame after a
/// change, and anchor events after pointer presses.
pub struct PageInput {
    watch: PageWatch,
    layouter: Box<dyn Layouter + Send>,
    /// Whether the layouter holds a layout of the current page.
    laid_out: bool,
    /// Buttons of the last pointer event.
    buttons: u8,
    /// The height event for the next frame.
    next: Option<Event>,
    /// The generation the layouter holds a layout of.
    laid: Option<u64>,
    /// Locates not yet answered: generation and anchor.
    pending: Vec<(u64, u16)>,
    /// The answers to the last frame's locates, for the next frame.
    located: Vec<Event>,
}

impl Default for PageInput {
    fn default() -> Self {
        Self::new(Box::new(ReferenceLayouter::default()))
    }
}

impl PageInput {
    pub fn new(layouter: Box<dyn Layouter + Send>) -> Self {
        PageInput { watch: PageWatch::default(), layouter, laid_out: false, buttons: 0, next: None, laid: None, pending: Vec::new(), located: Vec::new() }
    }

    /// The events for the next frame, given its other events: the pending
    /// height event first, then the answers to the last frame's locates,
    /// and after each pointer event that presses a button over anchored
    /// text, an anchor event (SPEC 5).
    pub fn events(&mut self, page: Option<&Page>, events: &[Event]) -> Vec<Event> {
        let mut out = Vec::with_capacity(events.len() + 1 + self.located.len());
        out.extend(self.next.take());
        out.append(&mut self.located);
        for &e in events {
            out.push(e);
            if e.kind != Event::POINTER {
                continue;
            }
            let (x, y, buttons) = e.pointer_parts();
            let pressed = buttons & !self.buttons != 0;
            self.buttons = buttons;
            if let Some(page) = page.filter(|_| pressed && self.laid_out) {
                let anchor = anchor_at(page, self.layouter.as_ref(), x, y);
                if anchor != 0 {
                    out.push(Event::page_anchor(anchor, buttons));
                }
            }
        }
        out
    }

    /// After a frame: lays the page out if it changed and keeps the height
    /// event and the answers to the frame's locates (those of the document
    /// current at its end) for the next frame.
    pub fn after_frame(&mut self, page: Option<&Page>) {
        let Some(page) = page else { return };
        self.located.clear();
        if let Some(key) = self.watch.wanted(page) {
            let height = match page.document() {
                Some(doc) => {
                    self.laid_out = true;
                    self.laid = Some(key.0);
                    self.layouter.layout(doc, key.1)
                }
                None => {
                    self.laid_out = false;
                    0
                }
            };
            if let Some(e) = self.watch.finished(key, height) {
                self.next = Some(e);
            }
        } else if page.document().is_none() {
            self.laid_out = false;
        }
        // A locate is answered once its document is laid out, and dropped
        // if the document is replaced first.
        self.pending.extend_from_slice(page.locates());
        self.pending.retain(|&(generation, _)| generation == page.generation());
        if self.laid == Some(page.generation()) {
            for (_, id) in self.pending.drain(..) {
                self.located.extend(Event::page_located(id, self.layouter.locate(id)));
            }
        }
        let n = self.pending.len();
        self.pending.drain(..n.saturating_sub(MAX_PENDING_LOCATES));
    }

    pub fn layouter(&self) -> &dyn Layouter {
        self.layouter.as_ref()
    }
}

/// The anchor under the screen position (x, y) on `page` as `layouter` last
/// laid it out; 0 if none or outside the rectangle.
pub fn anchor_at(page: &Page, layouter: &dyn Layouter, x: u16, y: u16) -> u16 {
    let r = page.region();
    if !r.visible() || x < r.x || y < r.y || x >= r.x + r.w || y >= r.y + r.h {
        return 0;
    }
    let scroll = page.scroll_for(layouter.height());
    layouter.anchor_at((x - r.x) as u32, (y - r.y) as u32 + scroll)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: impl AsRef<[u8]>) -> Vec<u8> {
        let s = s.as_ref();
        let mut v = vec![op::TEXT];
        v.extend_from_slice(&(s.len() as u16).to_le_bytes());
        v.extend_from_slice(s);
        v
    }

    #[test]
    fn region_is_clamped_to_the_screen() {
        let mut p = Page::default();
        p.set_region(600, 400, 100, 100);
        assert_eq!(p.region(), Region { x: 600, y: 400, w: 40, h: 80 });
        p.set_region(9999, 0, 5, 9999);
        assert_eq!(p.region(), Region { x: 640, y: 0, w: 0, h: 480 });
        assert!(!p.region().visible());
    }

    #[test]
    fn submit_rules() {
        let mut p = Page::default();
        assert_eq!(p.submit(&[]), RESULT_REJECTED);
        assert_eq!(p.submit(&vec![0; MAX_DOCUMENT + 1]), RESULT_REJECTED);
        assert_eq!(p.document(), None);
        p.set_scroll(50);
        assert_eq!(p.submit(&vec![0; MAX_DOCUMENT]), RESULT_DONE);
        assert_eq!(p.scroll(), 0);
        let g = p.generation();
        p.submit(&vec![0; MAX_DOCUMENT]);
        assert_eq!(p.generation(), g + 1, "the same bytes again are a new document");
        p.clear();
        assert_eq!(p.document(), None);
    }

    #[test]
    fn scroll_is_clamped_when_presented() {
        let mut p = Page::default();
        p.set_region(0, 0, 100, 100);
        p.set_scroll(-5i32 as u32);
        assert_eq!(p.scroll_for(500), 0);
        p.set_scroll(450);
        assert_eq!(p.scroll_for(500), 400);
        assert_eq!(p.scroll_for(50), 0);
    }

    #[test]
    fn parse_states_and_ends() {
        let mut doc = text(b"a\tb\x01c");
        doc.extend_from_slice(&[op::FONT, 7, 3, op::SIZE, 200, op::COLOUR, 1, 2, 3, op::ANCHOR, 5, 0]);
        doc.extend(text("d"));
        doc.extend_from_slice(&[op::PARAGRAPH, 1, 10, 20, 1]);
        doc.extend(text("\u{1}"));
        doc.extend_from_slice(&[op::RULE]);
        doc.extend(text(b"e\xff"));
        doc.extend_from_slice(&[op::TEXT, 9, 0, b'x']); // cut off
        let ps = parse(&doc);
        assert_eq!(ps.len(), 4);
        assert_eq!(ps[0].runs[0].text, "a bc");
        assert_eq!(
            ps[0].runs[1].style,
            Style { face: 0, bold: true, italic: true, size: 96, rgb: [1, 2, 3], anchor: 5 }
        );
        assert_eq!((ps[1].align, ps[1].indent, ps[1].space, ps[1].nowrap, ps[1].has_text()), (Align::Centre, 10, 20, true, false));
        assert_eq!(ps[2].rule, Some([1, 2, 3]));
        // Text after a rule: a paragraph like the latest Paragraph command.
        assert_eq!((ps[3].align, ps[3].space), (Align::Centre, 20));
        assert_eq!(ps[3].runs[0].text, "e\u{FFFD}");
        // Unknown opcode ends it.
        assert_eq!(parse(&[op::SIZE, 20, 0x99, op::RULE]).len(), 1);
        assert_eq!(parse(&[op::SIZE]).len(), 1);
    }

    #[test]
    fn reference_wraps_greedily() {
        // Size 10: 5 units per character; width 50: 10 characters a line.
        let mut doc = vec![op::SIZE, 10];
        doc.extend(text("aaa bbb ccc dddddddddddddddddddddd e"));
        let l = reference_layout(&doc, 50);
        let lines: Vec<String> = l.lines.iter().map(|l| l.spans.iter().map(|s| s.text.clone()).collect()).collect();
        assert_eq!(lines, ["aaa bbb", "ccc", "dddddddddd", "dddddddddd", "dd e"]);
        assert_eq!(l.lines[1].y, 13);
        assert_eq!(l.height, 5 * 13);
        // Wide characters take a whole size.
        let mut doc = vec![op::SIZE, 10];
        doc.extend(text("日本語x"));
        let l = reference_layout(&doc, 50);
        assert_eq!(l.lines[0].spans[0].x1, 2 * 35);
    }

    #[test]
    fn reference_paragraphs_rules_alignment() {
        let mut doc = vec![op::SIZE, 10];
        doc.extend(text("ab\n\nc"));
        doc.extend_from_slice(&[op::PARAGRAPH, 2, 4, 6, 0]);
        doc.extend(text("xy"));
        doc.extend_from_slice(&[op::RULE, op::PARAGRAPH, 1, 0, 0, 0]);
        doc.extend(text("z"));
        let l = reference_layout(&doc, 50);
        let ys: Vec<u32> = l.lines.iter().map(|l| l.y).collect();
        assert_eq!(ys, [0, 13, 26, 45, 58, 66]);
        assert_eq!(l.height, 79);
        // End aligned with indent 4: 100 half units, indent 8: 8 + 92 - 10.
        assert_eq!(l.lines[3].spans[0].x0, 8 + 92 - 10 * 2);
        assert_eq!(l.lines[4].rule, Some(DEFAULT_COLOUR));
        assert_eq!(l.lines[5].spans[0].x0, (100 - 10) / 2);
    }

    #[test]
    fn watch_reports_new_documents_and_heights() {
        let mut p = Page::default();
        let mut w = PageWatch::default();
        assert_eq!(w.wanted(&p), None);
        p.set_region(0, 0, 100, 100);
        assert_eq!(w.wanted(&p), None);
        p.submit(&text("hello"));
        let k = w.wanted(&p).unwrap();
        assert_eq!(w.finished(k, 18), Some(Event::page_laid_out(18)));
        assert_eq!(w.wanted(&p), None);
        // A width change with the same height: laid out, not reported.
        p.set_region(0, 0, 90, 100);
        let k = w.wanted(&p).unwrap();
        assert_eq!(w.finished(k, 18), None);
        p.set_region(0, 0, 10, 100);
        let k = w.wanted(&p).unwrap();
        assert_eq!(w.finished(k, 36), Some(Event::page_laid_out(36)));
        // Hidden by width 0: waits.
        p.set_region(0, 0, 0, 100);
        p.submit(&text("x"));
        assert_eq!(w.wanted(&p), None);
        p.clear();
        let k = w.wanted(&p).unwrap();
        assert_eq!(w.finished(k, 0), Some(Event::page_laid_out(0)));
        assert_eq!(w.finished((99, 1), 1 << 30), Some(Event::page_laid_out(MAX_REPORTED_HEIGHT)));
    }

    #[test]
    fn anchors_are_hit() {
        let mut doc = vec![op::SIZE, 10];
        doc.extend(text("ab "));
        doc.extend_from_slice(&[op::ANCHOR, 0x34, 0x12]);
        doc.extend(text("link"));
        let mut p = Page::default();
        p.set_region(100, 100, 200, 100);
        p.submit(&doc);
        let mut input = PageInput::default();
        input.after_frame(Some(&p));
        let ev = input.events(Some(&p), &[Event::pointer(100 + 16, 105, 1), Event::pointer(100 + 16, 105, 3), Event::pointer(100, 105, 1)]);
        assert_eq!(ev[0], Event::page_laid_out(13));
        assert_eq!(ev[2], Event::page_anchor(0x1234, 1));
        assert_eq!(ev[4], Event::page_anchor(0x1234, 3));
        assert_eq!(ev.len(), 6);
    }

    fn figure(w: u16, h: u16, align: u8, path: &[u8]) -> Vec<u8> {
        let mut v = vec![op::FIGURE];
        v.extend_from_slice(&w.to_le_bytes());
        v.extend_from_slice(&h.to_le_bytes());
        v.push(align);
        v.extend_from_slice(&(path.len() as u16).to_le_bytes());
        v.extend_from_slice(path);
        v
    }

    fn pt(op: u8, xy: &[i16]) -> Vec<u8> {
        let mut v = vec![op];
        for c in xy {
            v.extend_from_slice(&c.to_le_bytes());
        }
        v
    }

    #[test]
    fn figure_decodes_paths() {
        use path_op::*;
        let mut path = pt(LINE, &[160, 0]); // no Move: starts at (0, 0)
        path.extend(pt(QUAD, &[1, 2, 3, 4]));
        path.push(CLOSE);
        path.extend(pt(LINE, &[5, 6])); // after Close: from the start point
        path.extend_from_slice(&[FILL, 1, 2, 3, 4]);
        path.extend(pt(MOVE, &[-16, 32]));
        path.extend(pt(CUBIC, &[1, 2, 3, 4, 5, 6]));
        path.extend_from_slice(&[STROKE, 9, 9, 9, 0, 40, 0]); // a = 0: no stroke
        path.extend(pt(MOVE, &[0, 0])); // only a Move: draws nothing
        path.extend_from_slice(&[FILL, 0, 0, 0, 0]); // no fill and no stroke
        path.extend(pt(LINE, &[7, 7]));
        path.extend_from_slice(&[STROKE, 5, 6, 7, 8, 0, 1, 0x7F]); // 0x7F: unknown, ends the path
        path.extend(pt(LINE, &[1, 1]));
        let mut doc = vec![op::COLOUR, 10, 20, 30, op::ANCHOR, 3, 0];
        doc.extend(figure(100, 50, 1, &path));
        let ps = parse(&doc);
        assert_eq!(ps.len(), 1, "the figure took the empty first paragraph's place");
        let f = ps[0].figure.as_ref().unwrap();
        assert_eq!((f.width, f.height, f.rgb, f.anchor, ps[0].align), (100, 50, [10, 20, 30], 3, Align::Centre));
        let paths = f.paths();
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].fill, None);
        assert_eq!(paths[0].stroke, Some(([10, 20, 30, 255], 16)));
        use Segment::*;
        assert_eq!(paths[0].segments, [Move(0, 0), Line(160, 0), Quad(1, 2, 3, 4), Close, Move(0, 0), Line(5, 6)]);
        assert_eq!(paths[1].fill, Some([1, 2, 3, 4]));
        assert_eq!(paths[1].segments, [Move(-16, 32), Cubic(1, 2, 3, 4, 5, 6)]);
        // A cut-off path command ends the path; the commands before are used.
        let mut cut = pt(LINE, &[16, 16]);
        cut.extend_from_slice(&[CUBIC, 1, 0, 2]);
        let f = &parse(&figure(10, 10, 0, &cut))[0];
        assert_eq!(f.figure.as_ref().unwrap().paths()[0].segments, [Move(0, 0), Line(16, 16)]);
    }

    #[test]
    fn figure_limits_and_cut_off() {
        let ps = parse(&figure(0, 5000, 9, &[]));
        let f = ps[0].figure.as_ref().unwrap();
        assert_eq!((f.width, f.height, ps[0].align), (1, MAX_FIGURE_SIDE, Align::Start));
        // A Figure whose path bytes are cut off ends the document.
        let mut doc = text("a");
        let mut fig = figure(10, 10, 0, &[path_op::CLOSE; 4]);
        fig.truncate(fig.len() - 1);
        doc.extend(fig);
        let ps = parse(&doc);
        assert_eq!(ps.len(), 1);
        assert!(ps[0].figure.is_none());
        assert_eq!(parse(&[op::FIGURE, 1, 0, 1, 0, 0, 0]).len(), 1);
    }

    #[test]
    fn figure_area_cap() {
        use path_op::*;
        // A 100 x 100 figure may paint 80,000 square units: eight full-box fills.
        let mut path = vec![STROKE, 0, 0, 0, 0, 0, 0, FILL, 255, 0, 0, 255];
        for _ in 0..10 {
            path.extend(pt(MOVE, &[-160, -160]));
            path.extend(pt(LINE, &[3200, 3200]));
            path.extend(pt(LINE, &[0, 1600]));
            path.extend_from_slice(&[FILL, 255, 0, 0, 255]);
        }
        let f = Figure { width: 100, height: 100, rgb: [0; 3], anchor: 0, path };
        assert_eq!(f.area_cap(), 80_000);
        assert_eq!(f.paths().len(), 8);
        // A stroke counts its box grown by half its width; fill and stroke both count.
        let mut path = vec![FILL, 1, 1, 1, 255, STROKE, 1, 1, 1, 255, 32, 0];
        path.extend(pt(MOVE, &[160, 160]));
        path.extend(pt(LINE, &[320, 160]));
        let p = Figure { width: 100, height: 100, rgb: [0; 3], anchor: 0, path }.paths().remove(0);
        // Fill: 10 x 0 = 0; stroke: 12 x 2 units = 24.
        assert_eq!(p.area(100, 100), 24);
        // Length: a 1000-unit line stroked 1 unit is 2 × 1000 + 4 = 2,004;
        // filled too, twice that. 32 of the first fit in 65,536, 16 of the second.
        let line = |fill: bool| {
            let mut p = Vec::new();
            for _ in 0..40 {
                p.extend_from_slice(&[STROKE, 9, 9, 9, 255, 16, 0]);
                if fill {
                    p.extend_from_slice(&[FILL, 9, 9, 9, 255]);
                }
                p.extend(pt(MOVE, &[0, 0]));
                p.extend(pt(LINE, &[16_000, 0]));
            }
            Figure { width: 1000, height: 1000, rgb: [0; 3], anchor: 0, path: p }.paths()
        };
        assert_eq!(line(false)[0].length(), 2_004);
        assert_eq!(line(false).len(), 32);
        assert_eq!(line(true).len(), 16);
        // At most 256 paths.
        let mut path = Vec::new();
        for _ in 0..300 {
            path.extend_from_slice(&[FILL, 1, 1, 1, 255]);
            path.extend(pt(LINE, &[16, 16]));
        }
        assert_eq!(Figure { width: 100, height: 100, rgb: [0; 3], anchor: 0, path }.paths().len(), FIGURE_PATH_CAP);
        // Big figures are capped at four screens.
        let f = Figure { width: 1024, height: 1024, rgb: [0; 3], anchor: 0, path: vec![] };
        assert_eq!(f.area_cap(), FIGURE_AREA_CAP);
    }

    #[test]
    fn figure_reference_layout() {
        let mut doc = vec![op::SIZE, 10];
        doc.extend(text("ab"));
        doc.extend(figure(40, 30, 1, &[]));
        doc.extend(text("cd"));
        doc.extend_from_slice(&[op::PARAGRAPH, 0, 0, 7, 0]);
        doc.extend_from_slice(&[op::ANCHOR, 9, 0]);
        doc.extend(figure(300, 20, 2, &[]));
        doc.extend(text("ef"));
        let l = reference_layout(&doc, 100);
        let ys: Vec<(u32, u32)> = l.lines.iter().map(|l| (l.y, l.height)).collect();
        // The first figure follows "ab" with the implicit paragraph's space
        // (0); "cd" is a new paragraph; the second figure takes the empty
        // paragraph's place (space 7); "ef" gets space 7 again.
        assert_eq!(ys, [(0, 13), (13, 30), (43, 13), (63, 20), (90, 13)]);
        assert_eq!(l.height, 103);
        assert_eq!(l.lines[1].figure, Some(FigureBox { x: 30, width: 40, height: 30, shown: 40, anchor: 0 }));
        assert_eq!(l.lines[3].figure, Some(FigureBox { x: 0, width: 300, height: 20, shown: 100, anchor: 9 }));
        assert_eq!(l.anchor_at(99, 70), 9);
        assert_eq!(l.anchor_at(40, 20), 0);
        let mut p = Page::default();
        p.set_region(0, 0, 100, 200);
        p.submit(&doc);
        let dump = l.dump(&p);
        assert!(dump.contains("   13  30    30  -- figure 40x30 --\n"), "{dump}");
        assert!(dump.contains("   63  20     0  -- figure 300x20 -- {9}\n"), "{dump}");
    }

    /// A document with "a" (no anchor), "link" (anchor 3) on a line of its
    /// own, then a figure and text with anchor 5: lines (0, 18), (22, 18),
    /// (44, 30), (78, 18).
    fn anchored() -> Vec<u8> {
        let mut doc = text("a");
        doc.extend_from_slice(&[op::PARAGRAPH, 0, 0, 4, 0, op::ANCHOR, 3, 0]);
        doc.extend(text("link"));
        doc.extend_from_slice(&[op::ANCHOR, 5, 0]);
        doc.extend(figure(10, 30, 0, &[]));
        doc.extend(text("after"));
        doc
    }

    #[test]
    fn reference_locates_lines_and_figures() {
        let l = reference_layout(&anchored(), 100);
        let ys: Vec<(u32, u32)> = l.lines.iter().map(|l| (l.y, l.height)).collect();
        assert_eq!(ys, [(0, 18), (22, 18), (44, 30), (78, 18)]);
        assert_eq!(l.locate(3), Some((22, 18)));
        assert_eq!(l.locate(5), Some((44, 30)), "the figure is the first line of anchor 5");
        assert_eq!(l.locate(4), None);
    }

    #[test]
    fn locate_calls_and_answers() {
        let mut page = Page::default();
        let mut input = PageInput::default();
        assert_eq!(page.locate(3), RESULT_REJECTED, "no document");
        page.set_region(0, 0, 100, 50);
        page.submit(&anchored());
        assert_eq!(page.locate(0), RESULT_REJECTED);
        assert_eq!(page.locate(0x1_0003), RESULT_REJECTED);
        for id in [3, 5, 4] {
            assert_eq!(page.locate(id), RESULT_DONE);
        }
        input.after_frame(Some(&page));
        // Next frame: the height, then the answers in call order.
        page.begin_frame();
        let events = input.events(Some(&page), &[Event::key_down(4, 0)]);
        let mut want = vec![Event::page_laid_out(96)];
        want.extend(Event::page_located(3, Some((22, 18))));
        want.extend(Event::page_located(5, Some((44, 30))));
        want.extend(Event::page_located(4, None));
        want.push(Event::key_down(4, 0));
        assert_eq!(events, want);
        assert_eq!((want[5].c, [want[6].a, want[6].b, want[6].c]), (0, [0xFF; 3]), "absent: height 0, y FFFFFFh");
        // The document laid out already: answered at the next frame.
        assert_eq!(page.locate(3), RESULT_DONE);
        input.after_frame(Some(&page));
        page.begin_frame();
        assert_eq!(input.events(Some(&page), &[]), Event::page_located(3, Some((22, 18))));
        // Replaced (here later in the frame of the call): never answered.
        page.locate(3);
        page.submit(&text("x"));
        input.after_frame(Some(&page));
        page.begin_frame();
        assert_eq!(input.events(Some(&page), &[]), [Event::page_laid_out(18)]);
        // At most 8 a frame.
        for i in 0..MAX_LOCATES {
            assert_eq!(page.locate(1 + i as u32), RESULT_DONE);
        }
        assert_eq!(page.locate(1), RESULT_REJECTED);
        assert_eq!(page.locates().len(), MAX_LOCATES);
        let mut fresh = page.clone();
        fresh.begin_frame();
        let (mut a, mut b) = (Vec::new(), Vec::new());
        page.write_state(&mut a);
        fresh.write_state(&mut b);
        assert_eq!(a, b, "locates are not machine state");
        page.begin_frame();
        assert!(page.locates().is_empty());
        // While the width is 0 the answer waits.
        page.set_region(0, 0, 0, 50);
        page.submit(&anchored());
        page.locate(5);
        input.after_frame(Some(&page));
        page.begin_frame();
        assert_eq!(input.events(Some(&page), &[]), []);
        page.set_region(0, 0, 100, 50);
        input.after_frame(Some(&page));
        let mut want = vec![Event::page_laid_out(96)];
        want.extend(Event::page_located(5, Some((44, 30))));
        assert_eq!(input.events(Some(&page), &[]), want);
    }

    #[test]
    fn reference_ellipsis() {
        let doc = |flags: u8, s: &str| {
            let mut d = vec![op::PARAGRAPH, 0, 0, 0, flags, op::SIZE, 8];
            d.extend(text(s));
            d
        };
        let line = |d: &[u8]| -> String { reference_layout(d, 20).lines[0].spans.iter().map(|s| s.text.as_str()).collect() };
        // 20 units hold five characters of size 8 (4 units each).
        assert_eq!(line(&doc(3, "abcdefgh")), "abcd…");
        assert_eq!(line(&doc(3, "abcde")), "abcde", "it fits");
        assert_eq!(line(&doc(1, "abcdefgh")), "abcdefgh", "no wrap alone is cut where it is drawn");
        assert_eq!(line(&doc(2, "abcdefgh")), "abcde", "ellipsis without no wrap does nothing");
        assert_eq!(line(&doc(3, "ab\u{4E00}\u{4E00}\u{4E00}")), "ab\u{4E00}…");
        let l = reference_layout(&doc(3, "abcdefgh\nxy"), 20);
        assert_eq!(l.lines.len(), 2, "each line on its own");
    }

    #[test]
    fn state_covers_figures() {
        let state = |path: &[u8]| {
            let mut p = Page::default();
            p.submit(&figure(10, 10, 0, path));
            let mut out = Vec::new();
            p.write_state(&mut out);
            out
        };
        assert_ne!(state(&pt(path_op::LINE, &[1, 2])), state(&pt(path_op::LINE, &[1, 3])));
    }

    /// A table of `columns` with `flags` and alignments, then `cells` (each
    /// a Cell and its text), then End table.
    fn table(columns: u8, flags: u8, align: &[u8], cells: &[&str]) -> Vec<u8> {
        let mut v = vec![op::TABLE, columns, flags];
        v.extend_from_slice(align);
        for c in cells {
            v.push(op::CELL);
            if !c.is_empty() {
                v.extend(text(c));
            }
        }
        v.push(op::END_TABLE);
        v
    }

    #[test]
    fn table_parsing() {
        let mut doc = vec![op::SIZE, 10];
        doc.extend(text("before"));
        doc.extend_from_slice(&[op::COLOUR, 1, 2, 3, op::TABLE, 3, 1, 0, 1, 7]);
        doc.extend(text("ignored: before the first cell"));
        doc.extend_from_slice(&[op::CELL]);
        doc.extend(text("a"));
        doc.extend_from_slice(&[op::FONT, 2, 0]);
        doc.extend(text("b"));
        doc.extend_from_slice(&[op::CELL, op::CELL, op::FONT, 0, 2]);
        doc.extend(text("c"));
        doc.extend_from_slice(&[op::CELL]);
        doc.extend(text("d"));
        doc.extend_from_slice(&[op::END_TABLE, op::CELL]); // a Cell outside a table does nothing
        doc.extend(text("after"));
        let ps = parse(&doc);
        assert_eq!(ps.len(), 3);
        let t = ps[1].table.as_ref().unwrap();
        assert_eq!((t.columns, t.header, t.rgb, ps[1].space), (3, true, [1, 2, 3], 0));
        assert_eq!(t.align, [Align::Start, Align::Centre, Align::Start], "unknown alignment is start");
        assert_eq!(t.cells.len(), 4);
        assert_eq!(t.rows(), 2);
        // The header row is bold whatever its font says; other rows as given.
        assert_eq!(t.cells[0].iter().map(|r| (r.text.as_str(), r.style.face, r.style.bold)).collect::<Vec<_>>(), [("a", 0, true), ("b", 2, true)]);
        assert!(t.cells[1].is_empty());
        assert_eq!((t.cells[2][0].style.bold, t.cells[2][0].style.italic), (true, true));
        assert_eq!((t.cells[3][0].text.as_str(), t.cells[3][0].style.bold), ("d", false));
        assert_eq!(t.cell(1, 1), &[] as &[Run], "missing cells are empty");
        assert_eq!(ps[2].runs[0].text, "after", "text after a table starts a paragraph");
        // A Paragraph (Rule, Figure, Table) ends a table too; the table takes
        // the place of an empty paragraph and its space.
        let mut doc = vec![op::PARAGRAPH, 0, 0, 9, 0];
        doc.extend(&table(1, 0, &[0], &["x"])[..5 + 4]);
        doc.extend_from_slice(&[op::PARAGRAPH, 0, 0, 3, 0]);
        doc.extend(text("y"));
        doc.extend_from_slice(&[op::CELL]);
        doc.extend(text("z"));
        let ps = parse(&doc);
        assert_eq!(ps.len(), 3, "the implicit paragraph, the table, the paragraph");
        assert_eq!((ps[1].space, ps[1].table.as_ref().unwrap().cells.len()), (9, 1));
        assert_eq!(ps[2].runs[0].text, "yz");
        // Column counts 0 and 9 end the document; so does a cut-off Table.
        for (bad, more) in [(&[op::TABLE, 0, 0][..], true), (&[op::TABLE, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0], true), (&[op::TABLE, 2, 0, 0], false), (&[op::TABLE], false)] {
            let mut doc = text("a");
            doc.extend_from_slice(bad);
            if more {
                doc.extend(text("b"));
            }
            let ps = parse(&doc);
            assert_eq!((ps.len(), ps[0].runs[0].text.as_str()), (1, "a"), "{bad:?}");
        }
        assert_eq!(parse(&table(8, 0, &[2; 8], &[])).last().unwrap().table.as_ref().unwrap().align, [Align::End; 8]);
    }

    #[test]
    fn table_limits() {
        // 1,024 cells per table: the 1,025th Cell and the text after it are
        // dropped until the table ends.
        let cells: Vec<String> = (0..1_100).map(|i| i.to_string()).collect();
        let refs: Vec<&str> = cells.iter().map(String::as_str).collect();
        let mut doc = table(8, 0, &[0; 8], &refs);
        doc.extend(text("after"));
        let ps = parse(&doc);
        let t = ps[0].table.as_ref().unwrap();
        assert_eq!(t.cells.len(), MAX_TABLE_CELLS);
        assert_eq!(t.cells[1_023][0].text, "1023");
        assert_eq!(ps[1].runs[0].text, "after");
        // 4,096 per document: the fifth table of 1,000 cells gets 96.
        let thousand: Vec<&str> = refs[..1_000].to_vec();
        let mut doc = Vec::new();
        for _ in 0..5 {
            doc.extend(table(4, 0, &[0; 4], &thousand));
        }
        let counts: Vec<usize> = parse(&doc).iter().filter_map(|p| p.table.as_ref().map(|t| t.cells.len())).collect();
        assert_eq!(counts, [1_000, 1_000, 1_000, 1_000, 96]);
        assert!(doc.len() < MAX_DOCUMENT);
    }

    #[test]
    fn column_widths_shrink_the_widest_first() {
        // Fits: unchanged.
        assert_eq!(Table::column_widths(&[50, 100, 30], 200, 48), [50, 100, 30]);
        // 300 into 200: the cap is 85 (50 + 85 + 85 = 220 > 200; 30 + ...).
        assert_eq!(Table::column_widths(&[30, 200, 150], 200, 48), [30, 85, 85]);
        assert_eq!(Table::column_widths(&[30, 200, 100], 230, 48), [30, 100, 100]);
        // The floor: eight columns into 100 units get 48 each and are cut.
        assert_eq!(Table::column_widths(&[200; 8], 100, 48), [48; 8]);
        assert_eq!(Table::column_widths(&[20, 200, 200], 100, 48), [20, 48, 48]);
    }

    #[test]
    fn reference_table_layout() {
        // Size 10: 5 units per character, lines 13 high. Two columns, header.
        let mut doc = vec![op::SIZE, 10, op::COLOUR, 9, 9, 9];
        doc.extend(table(2, 1, &[0, 2], &["Name", "Value", "a", "1", "bb", "22"]));
        let l = reference_layout(&doc, 200);
        // Natural widths: "Name" 20 + 12 = 32, "Value" 25 + 12 = 37 (units).
        // Rows: 3 + 13 + 3 = 19 each; rules at 19 (header) and 38 (faint).
        let summary: Vec<(u32, u32, u32, String)> =
            l.lines.iter().map(|l| (l.y, l.height, l.spans.first().map_or(0, |s| s.x0), l.spans.iter().map(|s| s.text.clone()).collect())).collect();
        assert_eq!(
            summary,
            [
                (3, 13, 12, "Name".into()),
                (3, 13, 64 + 12, "Value".into()),
                (19, 0, 0, String::new()),
                (22, 13, 12, "a".into()),
                // End aligned: the column's text width is 25 units, "1" 5.
                (22, 13, 64 + 12 + 40, "1".into()),
                (38, 0, 0, String::new()),
                (41, 13, 12, "bb".into()),
                (41, 13, 64 + 12 + 30, "22".into()),
            ]
        );
        assert_eq!(l.height, 57);
        assert!(l.lines[0].spans[0].style.bold && !l.lines[3].spans[0].style.bold);
        assert_eq!(l.lines[2].rule_span, Some(RuleSpan { x0: 0, x1: 2 * 69, faint: false }));
        assert_eq!(l.lines[5].rule_span, Some(RuleSpan { x0: 0, x1: 2 * 69, faint: true }));
        assert_eq!(l.lines[2].rule, Some([9, 9, 9]));
        // Too wide: 100 units hold "a" (17) and a long column narrowed to 83
        // (71 of text: 14 characters a line), wrapped.
        let mut doc = vec![op::SIZE, 10];
        doc.extend(table(2, 0, &[0, 0], &["a", "one two three four five six seven"]));
        let l = reference_layout(&doc, 100);
        let lines: Vec<(u32, u32, String)> = l.lines.iter().map(|l| (l.y, l.spans[0].x0, l.spans.iter().map(|s| s.text.clone()).collect())).collect();
        assert_eq!(
            lines,
            [
                (3, 12, "a".into()),
                (3, 34 + 12, "one two three".into()),
                (16, 34 + 12, "four five six".into()),
                (29, 34 + 12, "seven".into()),
            ]
        );
        assert_eq!(l.height, 3 + 39 + 3);
        // No header, one row: no rule at all.
        assert!(l.lines.iter().all(|l| l.rule.is_none()));
        // Header with one row: a rule under it.
        let l = reference_layout(&table(1, 1, &[0], &["h"]), 100);
        assert_eq!(l.lines.last().unwrap().rule_span.map(|r| r.faint), Some(false));
        // An empty table takes only its space; an empty row only its padding.
        let mut doc = vec![op::PARAGRAPH, 0, 0, 7, 0];
        doc.extend(table(2, 0, &[0, 0], &[]));
        assert_eq!(reference_layout(&doc, 100).height, 7);
        assert_eq!(reference_layout(&table(2, 0, &[0, 0], &["", ""]), 100).height, 6);
    }

    #[test]
    fn table_anchors_are_hit_and_located() {
        let mut doc = vec![op::SIZE, 10];
        doc.extend(text("top"));
        doc.extend_from_slice(&[op::TABLE, 2, 0, 0, 0, op::CELL]);
        doc.extend(text("first line\nsecond "));
        doc.extend_from_slice(&[op::ANCHOR, 4, 0]);
        doc.extend(text("deep"));
        doc.extend_from_slice(&[op::ANCHOR, 0, 0, op::CELL, op::ANCHOR, 6, 0]);
        doc.extend(text("right"));
        doc.extend_from_slice(&[op::ANCHOR, 4, 0]);
        doc.extend(text(" also"));
        let l = reference_layout(&doc, 300);
        // The row starts at 13; its first lines at 16, the second at 29. The
        // first column is "second deep" wide: 55 + 12 = 67 units.
        assert_eq!(l.locate(6), Some((16, 13)));
        assert_eq!(l.locate(4), Some((16, 13)), "anchor 4 is on the right cell's first line, above 'deep'");
        assert_eq!(l.anchor_at(67 + 6 + 2, 20), 6);
        assert_eq!(l.anchor_at(6 + 7 * 5 + 2, 33), 4);
        assert_eq!(l.anchor_at(6 + 2, 33), 0);
        let mut page = Page::default();
        page.set_region(0, 0, 300, 100);
        page.submit(&doc);
        let dump = l.dump(&page);
        assert!(dump.contains("   29  13     6   sans10 |second [deep]{4}\n"), "{dump}");
    }

    #[test]
    fn state_and_snapshot_cover_tables() {
        let state = |doc: &[u8]| {
            let mut p = Page::default();
            p.submit(doc);
            let mut out = Vec::new();
            p.write_state(&mut out);
            out
        };
        let a = table(2, 1, &[0, 1], &["x", "y"]);
        let b = table(2, 1, &[0, 2], &["x", "y"]);
        assert_ne!(state(&a), state(&b));
    }
}
