//! What the page draws after a frame, as a list of drawing commands the
//! page's script plays on a canvas (`try.js`): each machine's screen in
//! its own screen units, then how the screens are put together (SPEC
//! §machines.screens). The canvas draws the text with the browser's fonts
//! and the shapes with its paths; everything about where things are comes
//! from here, from terse-core.
//!
//! A machine's screen is combined as SPEC §vector.layers says: the
//! terminal's cell backgrounds, the page (its background, its text and
//! figures as the reference layout places them, SPEC §page.reference),
//! the vector displays (device 3's shapes, then device 9's segments seen
//! from the reference eye), the terminal's characters and cursor. In the
//! phosphor style the backgrounds are not drawn and nothing is filled
//! (SPEC §vector.phosphor).
//!
//! Commands (little-endian; f32 coordinates; a colour is 4 bytes r g b a):
//!
//! - `01` screen: u32 key (the machine's serial; the shell is 0), u8
//!   flags (1: changed since last frame, 2: phosphor style), u8
//!   persistence; if changed, drawing commands follow up to `02`. A screen
//!   that did not change has no commands: the page keeps its last ones.
//! - `02` end.
//! - drawing: `0A` rect x y w h colour; `0B` line x0 y0 x1 y1 width
//!   colour; `0C` circle x y r width colour; `0D` disc x y r colour; `0E`
//!   triangle x0 y0 x1 y1 x2 y2 colour; `0F` text x baseline width size,
//!   u8 font (0 the terminal's, 1 sans, 2 serif, 3 mono), u8 flags (1
//!   bold, 2 italic), colour, u16 length, UTF-8; `10` clip x y w h; `11`
//!   unclip; `12` path: u8 filled, fill colour, u8 stroked, stroke colour,
//!   width, u16 count, then each segment: u8 kind (0 move, 1 line, 2
//!   quad, 3 cubic, 4 close) and its points.
//! - composing, in drawing order: `14` the screen of key over the whole
//!   picture; `15` flat: key, x y w h (shell units); `16` panel: key (0: a
//!   plate seen from behind), source x y w h (the machine's units), its
//!   four corners on the glass x y each (shell units); `17` drawing
//!   commands over the picture in shell units, up to `02`.

use std::borrow::Cow;
use std::collections::HashMap;

use terse_core::page::{FigurePath, Layout, Segment, face};
use terse_core::system_ink::Share;
use terse_core::vector::{decode_outlines_within, decode_within};
use terse_core::vector3::{self, Panel, SceneView};
use terse_core::{COLS, Machine, Primitive, ROWS, Terminal};

use crate::host::Host;

const SCREEN: u8 = 0x01;
const END: u8 = 0x02;
const RECT: u8 = 0x0A;
const LINE: u8 = 0x0B;
const CIRCLE: u8 = 0x0C;
const DISC: u8 = 0x0D;
const TRIANGLE: u8 = 0x0E;
const TEXT: u8 = 0x0F;
const CLIP: u8 = 0x10;
const UNCLIP: u8 = 0x11;
const PATH: u8 = 0x12;
const SHOW: u8 = 0x14;
const FLAT: u8 = 0x15;
const PANEL: u8 = 0x16;
const DIRECT: u8 = 0x17;

/// The fonts of [`TEXT`].
const FONT_TERMINAL: u8 = 0;

/// A cell of the terminal in screen units (SPEC §screen: 80 × 30 cells of
/// 8 × 16 units).
const CELL_W: f32 = 8.0;
const CELL_H: f32 = 16.0;

/// The eye 3D lists are seen from: the reference eye (no parallax yet).
const EYE: (f32, f32) = (0.0, 0.0);

#[derive(Default)]
pub struct Ops {
    pub buf: Vec<u8>,
}

impl Ops {
    fn op(&mut self, op: u8) {
        self.buf.push(op);
    }
    fn f(&mut self, v: f32) {
        self.buf.extend(v.to_le_bytes());
    }
    fn fs(&mut self, vs: &[f32]) {
        for &v in vs {
            self.f(v);
        }
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend(v.to_le_bytes());
    }
    fn colour(&mut self, c: [u8; 4]) {
        self.buf.extend(c);
    }
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
        self.op(RECT);
        self.fs(&[x, y, w, h]);
        self.colour(c);
    }
    fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: [u8; 4]) {
        self.op(LINE);
        self.fs(&[x0, y0, x1, y1, width]);
        self.colour(c);
    }
    #[allow(clippy::too_many_arguments)]
    fn text(&mut self, x: f32, baseline: f32, width: f32, size: f32, font: u8, flags: u8, c: [u8; 4], text: &str) {
        let bytes = &text.as_bytes()[..text.len().min(u16::MAX as usize)];
        self.op(TEXT);
        self.fs(&[x, baseline, width, size]);
        self.buf.extend([font, flags]);
        self.colour(c);
        self.buf.extend((bytes.len() as u16).to_le_bytes());
        self.buf.extend(bytes);
    }
    fn clip(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.op(CLIP);
        self.fs(&[x, y, w, h]);
    }
}

/// Draws what the system shows after its last frame into `out`, using and
/// updating `last` (each screen's commands as last given).
pub fn frame(host: &Host, last: &mut HashMap<u32, Vec<u8>>, out: &mut Ops) {
    out.buf.clear();
    let system = &host.system;
    let shares = system.shares();
    let scene = system.scene(EYE);
    let pictures = system.pictures();
    let serial_of = |h: u8| system.child(h).map(|c| c.serial);

    // The screens shown: the shell's, then each machine's with a picture or a panel.
    let mut shown: Vec<(u32, &Machine, Option<&Share>, bool)> = vec![(0, system.shell(), None, scene.is_none())];
    let panels: Vec<&Panel> = scene.iter().flat_map(|s| s.panels.iter()).filter(|p| p.handle != 0).collect();
    for (h, child) in system.children() {
        let Some(m) = &child.machine else { continue };
        if pictures.iter().any(|p| p.0 == h) || panels.iter().any(|p| p.handle == h) {
            shown.push((child.serial, m, shares.of(h), true));
        }
    }
    let mut screen = Ops::default();
    let mut keep = HashMap::new();
    for (key, machine, share, with3d) in shown {
        screen.buf.clear();
        draw_screen(&mut screen, machine, host.layout(key).as_deref(), share, with3d);
        let phosphor = machine.vector().is_some_and(|v| v.phosphor());
        let changed = last.get(&key) != Some(&screen.buf);
        out.op(SCREEN);
        out.u32(key);
        out.buf.push(changed as u8 | (phosphor as u8) << 1);
        out.buf.push(machine.vector().map_or(0, |v| v.persistence()));
        if changed {
            out.buf.extend_from_slice(&screen.buf);
            out.op(END);
        }
        keep.insert(key, std::mem::take(&mut screen.buf));
    }
    *last = keep;

    // Put together (SPEC §machines.screens).
    out.op(SHOW);
    out.u32(0);
    if let Some(scene) = &scene {
        let eye = eye_point(EYE);
        for p in &panels {
            let key = if p.front { serial_of(p.handle).unwrap_or(0) } else { 0 };
            out.op(PANEL);
            out.u32(key);
            out.fs(&p.source.map(|v| v as f32));
            for c in p.corners {
                let (x, y) = vector3::to_glass([c[0], c[1], c[2].max(vector3::NEAR_Z + 1.0)], eye);
                out.fs(&[x as f32, y as f32]);
            }
        }
        // The shell's 3D lines, over the panels, which hide what is behind them.
        let mut segments = Vec::new();
        vector3::project_scene(&system.shell().list3(), EYE, &scene.view, &mut segments, &mut Vec::new(), &mut Vec::new());
        out.op(DIRECT);
        for p in &segments {
            primitive(out, p);
        }
        out.op(END);
    }
    for (h, r) in pictures {
        let Some(key) = serial_of(h) else { continue };
        out.op(FLAT);
        out.u32(key);
        out.fs(&[r.x as f32, r.y as f32, r.w as f32, r.h as f32]);
    }
}

/// The eye at an offset from the reference eye (SPEC §vector3d.space).
fn eye_point(eye: (f32, f32)) -> (f64, f64) {
    let m = vector3::MAX_EYE_OFFSET;
    (vector3::EYE_X + eye.0.clamp(-m, m) as f64, vector3::EYE_Y + eye.1.clamp(-m, m) as f64)
}

/// The terminal a machine shows: its own (a blank one without the
/// device), with a fault on its bottom row, as terse-host shows it.
fn displayed_terminal(machine: &Machine) -> Cow<'_, Terminal> {
    let t = match machine.terminal() {
        Some(t) => Cow::Borrowed(t),
        None => Cow::Owned(Terminal::default()),
    };
    let Some(fault) = machine.fault() else { return t };
    let mut t = t.into_owned();
    t.set_colours(15, 1);
    t.set_cursor(0, ROWS as u32 - 1);
    t.command(0x04);
    t.write(format!(" fault: {fault}").as_bytes());
    t.command(0x06);
    Cow::Owned(t)
}

fn rgb(c: [u8; 3]) -> [u8; 4] {
    [c[0], c[1], c[2], 255]
}

/// One machine's screen in its own units.
fn draw_screen(out: &mut Ops, machine: &Machine, layout: Option<&Layout>, share: Option<&Share>, with3d: bool) {
    if share.is_some_and(|s| s.plate) {
        // SPEC §machines.ink: a picture that did not fit is a plain plate.
        out.rect(0.0, 0.0, 640.0, 480.0, [24, 24, 28, 255]);
        return;
    }
    let phosphor = machine.vector().is_some_and(|v| v.phosphor());
    let term = displayed_terminal(machine);
    let palette = term.palette();
    let cells: Vec<_> = (0..ROWS).map(|r| (0..COLS).map(|c| term.cell(c, r)).collect::<Vec<_>>()).collect();

    // 1. The cell backgrounds, in runs.
    if !phosphor {
        for (r, row) in cells.iter().enumerate() {
            let mut c = 0;
            while c < COLS {
                let bg = row[c].bg;
                let start = c;
                while c < COLS && row[c].bg == bg {
                    c += 1;
                }
                let colour = rgb(palette[bg as usize]);
                if colour != [0, 0, 0, 255] {
                    out.rect(start as f32 * CELL_W, r as f32 * CELL_H, (c - start) as f32 * CELL_W, CELL_H, colour);
                }
            }
        }
    }

    // 2. The page.
    if let (Some(page), Some(layout)) = (machine.page(), layout)
        && page.region().visible()
        && page.document().is_some()
    {
        draw_page(out, page, layout, phosphor);
    }

    // 3. The vector displays.
    let cap2 = share.map_or(terse_core::vector::INK_CAP, |s| s.cap2);
    let cap3 = share.map_or(vector3::INK_CAP, |s| s.cap3);
    let mut shapes = Vec::new();
    if let Some(v) = machine.vector() {
        if phosphor || v.outlined() {
            shapes.extend(decode_outlines_within(v.list(), cap2));
        } else {
            shapes.extend(decode_within(v.list(), cap2));
        }
    }
    if with3d && let Some(v3) = machine.vector3().filter(|v| !v.list().is_empty()) {
        vector3::project_scene_within(v3.list(), EYE, &SceneView::LISTED, &mut shapes, &mut Vec::new(), &mut Vec::new(), cap3);
    }
    for p in &shapes {
        primitive(out, p);
    }

    // 4. The characters and the cursor.
    for (r, row) in cells.iter().enumerate() {
        let baseline = r as f32 * CELL_H + 12.5;
        let mut c = 0;
        while c < COLS {
            let wide = c + 1 < COLS && row[c + 1].ch == terse_core::CONTINUATION;
            if wide {
                let s = cell_char(row[c].ch).to_string();
                out.text(c as f32 * CELL_W, baseline, 2.0 * CELL_W, 13.0, FONT_TERMINAL, 0, rgb(palette[row[c].fg as usize]), &s);
                c += 2;
                continue;
            }
            let fg = row[c].fg;
            let start = c;
            let mut s = String::new();
            while c < COLS
                && row[c].fg == fg
                && row[c].ch != terse_core::CONTINUATION
                && !(c + 1 < COLS && row[c + 1].ch == terse_core::CONTINUATION)
            {
                s.push(cell_char(row[c].ch));
                c += 1;
            }
            if start == c {
                // A stray continuation cell.
                c += 1;
                continue;
            }
            let trimmed = s.trim_end_matches(' ');
            let lead = trimmed.len() - trimmed.trim_start_matches(' ').len();
            let n = trimmed.chars().count() - lead;
            if n > 0 {
                let x = (start + lead) as f32 * CELL_W;
                out.text(x, baseline, n as f32 * CELL_W, 13.0, FONT_TERMINAL, 0, rgb(palette[fg as usize]), &trimmed[lead..]);
            }
        }
    }
    if term.shows_cursor() {
        let (c, r) = term.cursor();
        let cell = term.cell(c as usize, r as usize);
        out.rect(c as f32 * CELL_W, r as f32 * CELL_H + CELL_H - 2.0, CELL_W, 2.0, rgb(palette[cell.fg as usize]));
    }
}

/// The character a cell shows: its code, a space for a control character
/// or none, U+FFFD for a surrogate.
fn cell_char(ch: u16) -> char {
    match ch {
        0..0x20 | 0x7F..0xA0 => ' ',
        c => char::from_u32(c as u32).unwrap_or('\u{FFFD}'),
    }
}

/// A shape of a vector list (coordinates in sixteenths of a unit).
fn primitive(out: &mut Ops, p: &Primitive) {
    let u = |v: i16| v as f32 / 16.0;
    let w = |v: u8| v.max(1) as f32 / 16.0;
    match *p {
        Primitive::Line { x0, y0, x1, y1, width, rgba } => out.line(u(x0), u(y0), u(x1), u(y1), w(width), rgba),
        Primitive::Beam { x0, y0, x1, y1, width, rgba, fade, .. } => {
            let a = (rgba[3] as u32 * (fade[0] as u32 + fade[1] as u32) / 510) as u8;
            out.line(u(x0), u(y0), u(x1), u(y1), w(width), [rgba[0], rgba[1], rgba[2], a]);
        }
        Primitive::Circle { x, y, radius, width, rgba } => {
            out.op(CIRCLE);
            out.fs(&[u(x), u(y), radius as f32 / 16.0, w(width)]);
            out.colour(rgba);
        }
        Primitive::Disc { x, y, radius, rgba } => {
            out.op(DISC);
            out.fs(&[u(x), u(y), radius as f32 / 16.0]);
            out.colour(rgba);
        }
        Primitive::Rect { x0, y0, x1, y1, rgba } => {
            let (l, t) = (u(x0.min(x1)), u(y0.min(y1)));
            out.rect(l, t, u(x0.max(x1)) - l, u(y0.max(y1)) - t, rgba);
        }
        Primitive::Triangle { x0, y0, x1, y1, x2, y2, rgba } => {
            out.op(TRIANGLE);
            out.fs(&[u(x0), u(y0), u(x1), u(y1), u(x2), u(y2)]);
            out.colour(rgba);
        }
    }
}

fn font_of(f: u8) -> u8 {
    match f {
        face::SERIF => 2,
        face::MONO => 3,
        _ => 1,
    }
}

/// The page (SPEC §page): its background (not in the phosphor style), then
/// the visible lines of the reference layout, clipped to its rectangle.
fn draw_page(out: &mut Ops, page: &terse_core::Page, layout: &Layout, phosphor: bool) {
    let r = page.region();
    let (rx, ry, rw, rh) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
    if !phosphor {
        let bg = page.background();
        if bg[3] != 0 {
            out.rect(rx, ry, rw, rh, bg);
        }
    }
    out.clip(rx, ry, rw, rh);
    let scroll = page.scroll_for(layout.height);
    // The figures, in document order: the nth figure line shows the nth.
    let figures: Vec<_> = match page.document() {
        Some(doc) => terse_core::page::parse(doc).into_iter().filter_map(|p| p.figure).collect(),
        None => Vec::new(),
    };
    let mut nth = 0;
    for line in &layout.lines {
        let is_figure = line.figure.is_some();
        let visible = line.y < scroll + r.h as u32 && line.y + line.height.max(1) > scroll;
        if !visible {
            nth += is_figure as usize;
            continue;
        }
        let top = ry + line.y as f32 - scroll as f32;
        if let Some(c) = line.rule {
            match line.rule_span {
                Some(s) => {
                    let a = if s.faint { terse_core::page::FAINT_RULE_ALPHA } else { 255 };
                    out.line(rx + s.x0 as f32 / 2.0, top, rx + s.x1 as f32 / 2.0, top, 1.0, [c[0], c[1], c[2], a]);
                }
                None => {
                    let y = top + line.height as f32 / 2.0;
                    out.line(rx, y, rx + rw, y, 1.0, rgb(c));
                }
            }
        }
        if let Some(b) = line.figure {
            if let Some(f) = figures.get(nth) {
                let fx = rx + b.x as f32;
                out.clip(fx, top, b.shown as f32, b.height as f32);
                for p in f.paths() {
                    figure_path(out, &p, fx, top, phosphor);
                }
                out.op(UNCLIP);
            }
            nth += 1;
        }
        for s in &line.spans {
            let size = s.style.size as f32;
            let baseline = top + line.height as f32 / 2.0 + 0.35 * size;
            let flags = s.style.bold as u8 | (s.style.italic as u8) << 1;
            let x = rx + s.x0 as f32 / 2.0;
            let w = (s.x1 - s.x0) as f32 / 2.0;
            out.text(x, baseline, w, size, font_of(s.style.face), flags, rgb(s.style.rgb), &s.text);
        }
    }
    out.op(UNCLIP);
}

/// A figure's path at (fx, fy); its points are in sixteenths of a unit.
/// In the phosphor style it is outlined (SPEC §vector.phosphor).
fn figure_path(out: &mut Ops, p: &FigurePath, fx: f32, fy: f32, phosphor: bool) {
    let (mut fill, mut stroke) = (p.fill, p.stroke);
    if phosphor && let Some(f) = fill.take() {
        stroke.get_or_insert(([f[0], f[1], f[2], 255], 16));
    }
    out.op(PATH);
    out.buf.push(fill.is_some() as u8);
    out.colour(fill.unwrap_or_default());
    out.buf.push(stroke.is_some() as u8);
    out.colour(stroke.map_or([0; 4], |s| s.0));
    out.f(stroke.map_or(0.0, |s| s.1 as f32 / 16.0));
    out.buf.extend((p.segments.len().min(u16::MAX as usize) as u16).to_le_bytes());
    let pt = |out: &mut Ops, x: i16, y: i16| out.fs(&[fx + x as f32 / 16.0, fy + y as f32 / 16.0]);
    for s in p.segments.iter().take(u16::MAX as usize) {
        match *s {
            Segment::Move(x, y) => {
                out.buf.push(0);
                pt(out, x, y);
            }
            Segment::Line(x, y) => {
                out.buf.push(1);
                pt(out, x, y);
            }
            Segment::Quad(a, b, x, y) => {
                out.buf.push(2);
                pt(out, a, b);
                pt(out, x, y);
            }
            Segment::Cubic(a, b, c, d, x, y) => {
                out.buf.push(3);
                pt(out, a, b);
                pt(out, c, d);
                pt(out, x, y);
            }
            Segment::Close => out.buf.push(4),
        }
    }
}

/// The text of what the screens show, for tests: each screen's text
/// commands in order.
pub fn texts(ops: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < ops.len() {
        let op = ops[at];
        at += 1;
        at += match op {
            SCREEN => 6,
            END | UNCLIP => 0,
            RECT => 20,
            LINE => 24,
            CIRCLE => 20,
            DISC => 16,
            TRIANGLE => 28,
            TEXT => {
                let len = u16::from_le_bytes([ops[at + 22], ops[at + 23]]) as usize;
                out.push(String::from_utf8_lossy(&ops[at + 24..at + 24 + len]).into_owned());
                24 + len
            }
            CLIP => 16,
            PATH => {
                let n = u16::from_le_bytes([ops[at + 14], ops[at + 15]]) as usize;
                let mut k = at + 16;
                for _ in 0..n {
                    let kind = ops[k];
                    k += 1 + 8 * [1, 1, 2, 3, 0][kind as usize];
                }
                k - at
            }
            SHOW => 4,
            FLAT => 20,
            PANEL => 4 + 16 + 32,
            DIRECT => 0,
            _ => panic!("unknown command {op:#x} at {}", at - 1),
        };
    }
    out
}
