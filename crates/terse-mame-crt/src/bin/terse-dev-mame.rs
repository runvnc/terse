//! `terse-dev-mame`: the vector displays (slots 3 and 9) or the terminal
//! (slot 2), whichever group the core asks for, drawn in the phosphor style
//! with MAME's vector CRT pipeline (`terse_mame_crt::pipeline`).
//!
//! The devices are terse-core's, as in the reference drivers
//! (`terse-dev-vector`, `terse-dev-terminal`): the same calls, results,
//! charges and state, so a machine runs the same whichever driver draws it.
//! Only the picture differs:
//!
//! - **The clean style** is drawn as the reference drivers draw it: the
//!   lists with terse-host's shapes; the terminal's cell backgrounds, and
//!   its characters as strokes of the stroke font (not bitmaps).
//! - **The phosphor style** (`core.light`): each group draws its own beam
//!   with the MAME pipeline, one phosphor per machine, into a light layer
//!   (DEVICES.md 7.1), and publishes `vector.tube` = 1 so that the core
//!   adds that light as it is instead of drawing its own tube (DEVICES.md
//!   7.2). The vector group's beam is the 2D list then the 3D list's
//!   segments; the terminal's is its text as strokes, from the top left.
//!   Each group has its own phosphor: the two pictures are added by the
//!   core.
//!
//! Without a graphics adapter it says so and draws the phosphor style as
//! the reference drivers do (the vector layers empty, the core's beam; the
//! terminal's strokes as plain light) and publishes `vector.tube` = 0.
//!
//! Run with `terse-bus <image> --driver 3,9=terse-dev-mame --driver
//! 2=terse-dev-mame`; install with `terse drivers add --slot 3 <file>` and
//! `--slot 2`. `TERSE_MAME_TIMES=1` prints the pipeline's frame times at
//! exit; `TERSE_MAME_*` environment variables set MAME's sliders
//! (`Params::from_env`).

use std::collections::HashMap;

use terse_bus::driver::{Begin, CallIn, Device, Inputs, LayerOut, Mem, Present, PresentOut, Reply, Session};
use terse_bus::proto::{CallEntry, DeviceInfo, LayerDecl, Open, SlotInfo, access, property, rule, z};
use terse_core::machine::{charge, copy_charge};
use terse_core::terminal::{CELL_BYTES, CELLS};
use terse_core::vector3::{SLOT as SLOT3, SceneView, Transform};
use terse_core::{COLS, ERR_BUFFER, ERR_UNKNOWN, Primitive, ROWS, Terminal, Vector, Vector3};
use terse_host::display::decode_shapes_within;
use terse_mame_crt::beams::Line;
use terse_mame_crt::layers::{Drawn, Times, Tube, TubeLook, draw_clean, draw_empty, draw_tube};
use terse_mame_crt::pipeline::{Gpu, Params};
use terse_mame_crt::text;

/// Published by a vector driver that draws the phosphor style itself
/// (DEVICES.md 7.2): 1 when it does.
const TUBE: &str = "vector.tube";
const NAME: &str = "terse-dev-mame";

/// Device 3's calls (DEVICES.md 5.1), as terse-dev-vector's.
fn table2() -> Vec<CallEntry> {
    let e = CallEntry::plain;
    vec![
        e(0).buffer(0, 1, 1, 1, terse_core::vector::MAX_LIST as u32, access::READ).results(1, 1).batch(rule::CONSTANT, 0, 0, 0, 0).per4(),
        e(1).batch(rule::CONSTANT, 0, 0, 0, 0),
        e(2).batch(rule::AT_MOST, 0, 1, 0, 0),
        e(3).batch(rule::AT_MOST, 0, 255, 0, 0),
        e(4).batch(rule::AT_MOST, 0, 1, 0, 0),
    ]
}

/// Device 9's calls, as terse-dev-vector's.
fn table3() -> Vec<CallEntry> {
    let e = CallEntry::plain;
    vec![
        e(0).buffer(0, 1, 1, 1, terse_core::vector3::MAX_LIST as u32, access::READ).results(1, 1).batch(rule::CONSTANT, 0, 0, 0, 0).per4(),
        e(1).batch(rule::CONSTANT, 0, 0, 0, 0),
        e(2).batch(rule::AT_MOST, 0, 1, 0, 0),
        e(3).batch(rule::AT_MOST, 0, 255, 0, 0),
    ]
}

/// The terminal's calls, as terse-dev-terminal's.
fn table_terminal() -> Vec<CallEntry> {
    let e = CallEntry::plain;
    vec![
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

const FULL_CAPS: (u64, f64) = (terse_core::vector::INK_CAP, terse_core::vector3::INK_CAP);

fn decode_caps(b: &[u8]) -> (u64, f64) {
    if b.len() != 16 {
        return FULL_CAPS;
    }
    let c2 = u64::from_le_bytes(b[..8].try_into().unwrap());
    let c3 = f64::from_le_bytes(b[8..].try_into().unwrap());
    (c2.min(FULL_CAPS.0), if c3.is_nan() { FULL_CAPS.1 } else { c3.clamp(0.0, FULL_CAPS.1) })
}

fn decode_view(b: &[u8]) -> SceneView {
    if b.len() != 1 + 12 * 8 {
        return SceneView::LISTED;
    }
    let f = |i: usize| f64::from_le_bytes(b[1 + 8 * i..9 + 8 * i].try_into().unwrap());
    let m = [[f(0), f(1), f(2)], [f(3), f(4), f(5)], [f(6), f(7), f(8)]];
    SceneView { screens: b[0], view: Transform { m, t: [f(9), f(10), f(11)] } }
}

/// One machine's vector displays.
struct VInst {
    v: Vector,
    v3: Option<Vector3>,
    lists2: bool,
    eye: (f32, f32),
    view: SceneView,
    caps: (u64, f64),
    light: bool,
    /// The host's shed level (`core.shed`, DEVICES.md 7.2).
    shed: u8,
    frames: u64,
    drawn: [Option<Drawn>; 2],
    tube: Option<Tube>,
}

impl VInst {
    fn outlines(&self) -> bool {
        self.v.outlined() || self.v.phosphor()
    }

    /// The 2D list's primitives (as outlines in the phosphor style) and
    /// the 3D list's segments.
    fn primitives(&self, k: usize, outlines: bool) -> Vec<Primitive> {
        let mut out = Vec::new();
        match k {
            0 => decode_shapes_within(self.v.list(), outlines, self.caps.0, &mut out),
            _ => {
                if let Some(v3) = self.v3.as_ref().filter(|v| !v.list().is_empty()) {
                    terse_core::vector3::project_scene_within(v3.list(), self.eye, &self.view, &mut out, &mut Vec::new(), &mut Vec::new(), self.caps.1);
                }
            }
        }
        out
    }
}

/// One machine's terminal.
struct TInst {
    t: Terminal,
    status: String,
    light: bool,
    /// The host's shed level (`core.shed`).
    shed: u8,
    frames: u64,
    /// Backgrounds (clean style) and characters.
    drawn: [Option<Drawn>; 2],
    bg_drawn: Option<(Vec<u8>, [[u8; 3]; 256])>,
    tube: Option<Tube>,
}

#[derive(Clone, Copy, PartialEq)]
enum Group {
    Vector,
    Terminal,
}

struct Driver {
    group: Group,
    presents: bool,
    slots: Vec<u8>,
    gpu: Option<Gpu>,
    params: Params,
    /// The look at shed level 0 (`TubeLook::from_env`).
    look: TubeLook,
    vinst: HashMap<u32, VInst>,
    tinst: HashMap<u32, TInst>,
    times: Times,
}

const BG: u8 = 0;
const FG: u8 = 1;

impl Driver {
    fn present_vector(&mut self, instance: u32, p: &Present, session: &Session) -> PresentOut {
        let Some(i) = self.vinst.get_mut(&instance) else { return PresentOut::default() };
        let mut out = PresentOut::default();
        let on = [(0usize, 3u8, i.lists2 && self.slots.contains(&3)), (1, SLOT3, i.v3.is_some())];
        // In the phosphor style, the whole beam goes into the first layer.
        let beam_layer = on.iter().find(|o| o.2).map(|o| o.0);
        for (k, slot, present) in on {
            if !present {
                continue;
            }
            let r = match (&self.gpu, i.light) {
                (Some(gpu), true) if Some(k) == beam_layer => {
                    let prims: Vec<Primitive> = i.primitives(0, true).into_iter().chain(i.primitives(1, true)).collect();
                    let (drawn, tube) = (&mut i.drawn[k], &mut i.tube);
                    let look = TubeLook::at(i.shed, self.look.max_lines, self.look.delivery);
                    draw_tube(drawn, tube, gpu, self.params, look, p, session.dir(), instance, slot, 0, i.frames, |s| {
                        let mut lines = Vec::new();
                        s.lines(&prims, &mut lines);
                        lines
                    }, &mut self.times)
                }
                (_, true) => draw_empty(&mut i.drawn[k], p, session.dir(), instance, slot, 0),
                (_, false) => {
                    let prims = i.primitives(k, i.outlines());
                    draw_clean(&mut i.drawn[k], p, session.dir(), instance, slot, 0, &prims)
                }
            };
            match r {
                Ok(l) => out.layers.push(l),
                Err(e) => eprintln!("{NAME}: {e}"),
            }
        }
        if !i.light {
            i.tube = None;
        }
        out
    }

    fn present_terminal(&mut self, instance: u32, p: &Present, session: &Session) -> PresentOut {
        let Some(i) = self.tinst.get_mut(&instance) else { return PresentOut::default() };
        let t = text::shown(&i.t, &i.status);
        let mut out = PresentOut::default();
        let dir = session.dir();
        // Backgrounds: none in the phosphor style; else each cell's.
        let bg = if i.light {
            i.bg_drawn = None;
            draw_empty(&mut i.drawn[0], p, dir, instance, 2, BG)
        } else {
            let cells = text::cells(&t);
            let key: Vec<u8> = cells.iter().map(|c| c.3).collect();
            let palette = *t.palette();
            let fresh = i.drawn[0].as_ref().is_none_or(|d| d.picture != p.picture || d.screen != p.screen);
            if !fresh && i.bg_drawn.as_ref().is_some_and(|(k, pl)| *k == key && *pl == palette) {
                Ok(LayerOut { slot: 2, id: BG, new_file: None, damage: Vec::new() })
            } else {
                i.bg_drawn = Some((key.clone(), palette));
                // A rectangle per cell, its background colour, opaque.
                let rects: Vec<Primitive> = (0..ROWS * COLS)
                    .map(|n| {
                        let (col, row) = ((n % COLS) as i16, (n / COLS) as i16);
                        let [r, g, b] = palette[key[n] as usize];
                        Primitive::Rect { x0: col * 128, y0: row * 256, x1: (col + 1) * 128, y1: (row + 1) * 256, rgba: [r, g, b, 255] }
                    })
                    .collect();
                draw_clean(&mut i.drawn[0], p, dir, instance, 2, BG, &rects)
            }
        };
        let lit = text::screen(&t);
        let fg = match (&self.gpu, i.light) {
            (Some(gpu), true) => draw_tube(&mut i.drawn[1], &mut i.tube, gpu, self.params, TubeLook::at(i.shed, self.look.max_lines, self.look.delivery), p, dir, instance, 2, FG, i.frames, |s| {
                lit.iter()
                    .map(|l| {
                        let st = l.s;
                        let level = (st.level * 255.0).round() as u8;
                        s.line(st.x0 * 16.0, st.y0 * 16.0, st.x1 * 16.0, st.y1 * 16.0, st.width * 16.0, [l.rgb[0], l.rgb[1], l.rgb[2], level], 1.0)
                    })
                    .collect::<Vec<Line>>()
            }, &mut self.times),
            _ => {
                // Plain strokes, antialiased, in the clean style's way (and
                // as plain light without a graphics adapter).
                let prims: Vec<Primitive> = lit
                    .iter()
                    .map(|l| {
                        let q = |v: f32| (v * 16.0).round() as i16;
                        let s = l.s;
                        let a = (s.level * 255.0).round() as u8;
                        Primitive::Line { x0: q(s.x0), y0: q(s.y0), x1: q(s.x1), y1: q(s.y1), width: (s.width * 16.0).round() as u8, rgba: [l.rgb[0], l.rgb[1], l.rgb[2], a] }
                    })
                    .collect();
                let r = draw_clean(&mut i.drawn[1], p, dir, instance, 2, FG, &prims);
                if i.light {
                    // Light (DEVICES.md 7.1): straight colour, coverage in the
                    // alpha. The clean layer is premultiplied: undo it.
                    if let Some(d) = &mut i.drawn[1] {
                        for q in d.layer.pixels() {
                            let a = *q >> 24;
                            if a != 0 && a != 255 {
                                let un = |c: u32| ((c * 255 + a / 2) / a).min(255);
                                *q = a << 24 | un(*q >> 16 & 255) << 16 | un(*q >> 8 & 255) << 8 | un(*q & 255);
                            }
                        }
                    }
                }
                r
            }
        };
        if !i.light {
            i.tube = None;
        }
        for r in [bg, fg] {
            match r {
                Ok(l) => out.layers.push(l),
                Err(e) => eprintln!("{NAME}: {e}"),
            }
        }
        out
    }
}

impl Device for Driver {
    fn info(&mut self, session: &Session) -> DeviceInfo {
        self.presents = session.presents();
        self.slots = session.hello.slots.clone();
        self.group = if session.asks(3) || session.asks(SLOT3) { Group::Vector } else { Group::Terminal };
        if self.presents {
            match Gpu::new() {
                Ok(g) => {
                    eprintln!("{NAME}: MAME vector CRT pipeline on {}{}", g.adapter, if g.software { " (a software adapter)" } else { "" });
                    self.gpu = Some(g);
                }
                Err(e) => eprintln!("{NAME}: {e}; the phosphor style is drawn as the reference drivers draw it"),
            }
        }
        let layer = |zv| if self.presents { vec![LayerDecl { id: 0, z: zv }] } else { Vec::new() };
        let mut slots = Vec::new();
        let (publishes, subscribes) = match self.group {
            Group::Vector => {
                if session.asks(3) {
                    slots.push(SlotInfo { slot: 3, version: 0, layers: layer(z::VECTOR), table: table2() });
                }
                if session.asks(SLOT3) {
                    slots.push(SlotInfo { slot: SLOT3, version: 0, layers: layer(z::VECTOR3), table: table3() });
                }
                (
                    vec![property::OUTLINES.into(), property::LIST3.into(), property::LIST2.into(), TUBE.into()],
                    vec![property::EYE.into(), property::VIEW.into(), property::CAPS.into(), property::LIGHT.into(), property::SHED.into()],
                )
            }
            Group::Terminal => {
                // Slot 2 only: the stroke font does not draw every
                // character, so it does not give full text (slot 10).
                let layers = if self.presents {
                    vec![LayerDecl { id: BG, z: z::TERMINAL_BACKGROUNDS }, LayerDecl { id: FG, z: z::TERMINAL_CHARACTERS }]
                } else {
                    Vec::new()
                };
                if session.asks(2) {
                    slots.push(SlotInfo { slot: 2, version: 0, layers, table: table_terminal() });
                }
                (Vec::new(), vec![property::STATUS.into(), property::LIGHT.into(), property::SHED.into()])
            }
        };
        DeviceInfo {
            name: format!("{NAME} (MAME vector CRT, {})", if self.group == Group::Vector { "vector" } else { "terminal" }),
            // The terminal's native size: 80 cells of 8 pixels, 4:3.
            native: if self.group == Group::Terminal && self.presents { (640, 480) } else { (0, 0) },
            slots,
            publishes,
            subscribes,
            services: Vec::new(),
            ..Default::default()
        }
    }

    fn open(&mut self, instance: u32, open: &Open, _: &Session) -> Result<(), String> {
        match self.group {
            Group::Vector => {
                if open.lists(SLOT3) && !self.slots.contains(&3) {
                    return Err("device 9 needs this driver to serve device 3 too (SPEC §vector3d.calls)".into());
                }
                let v = VInst {
                    v: Vector::default(),
                    v3: open.lists(SLOT3).then(Vector3::default),
                    lists2: open.lists(3),
                    eye: (0.0, 0.0),
                    view: SceneView::LISTED,
                    caps: FULL_CAPS,
                    light: false,
                    shed: 0,
                    frames: 0,
                    drawn: [None, None],
                    tube: None,
                };
                self.vinst.insert(instance, v);
            }
            Group::Terminal => {
                // Without full text: the basic repertoire (SPEC §terminal.text).
                let t = TInst { t: Terminal::with_full_text(false), status: String::new(), light: false, shed: 0, frames: 0, drawn: [None, None], bg_drawn: None, tube: None };
                self.tinst.insert(instance, t);
            }
        }
        Ok(())
    }

    fn close(&mut self, instance: u32) {
        self.vinst.remove(&instance);
        self.tinst.remove(&instance);
    }

    fn reset(&mut self, instance: u32) {
        if let Some(i) = self.vinst.get_mut(&instance) {
            i.v = Vector::default();
            if let Some(v3) = &mut i.v3 {
                *v3 = Vector3::default();
            }
        }
        if let Some(i) = self.tinst.get_mut(&instance) {
            i.t = Terminal::with_full_text(false);
        }
    }

    fn call(&mut self, instance: u32, c: &CallIn, mem: &mut dyn Mem) -> Reply {
        if let Some(i) = self.vinst.get_mut(&instance) {
            let (a0, charge) = match (c.slot, c.function) {
                (3, 0) => {
                    let r = i.v.submit(c.buf);
                    (r, if r == 0 { copy_charge(c.buf.len()) } else { 0 })
                }
                (3, 1) => {
                    i.v.clear();
                    (0, 0)
                }
                (SLOT3, 0) if i.v3.is_some() => {
                    let r = i.v3.as_mut().unwrap().submit(c.buf);
                    (r, if r == 0 { copy_charge(c.buf.len()) } else { 0 })
                }
                (SLOT3, 1) if i.v3.is_some() => {
                    i.v3.as_mut().unwrap().clear();
                    (0, 0)
                }
                (3 | SLOT3, 2) => (i.v.set_style(c.args[0]), 0),
                (3 | SLOT3, 3) => (i.v.set_persistence(c.args[0]), 0),
                (3, 4) => (i.v.set_fill(c.args[0]), 0),
                _ => (ERR_UNKNOWN, 0),
            };
            return Reply { a0, a1: None, charge };
        }
        let Some(i) = self.tinst.get_mut(&instance) else { return Reply { a0: ERR_UNKNOWN, ..Default::default() } };
        // terse-dev-terminal's calls, exactly.
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

    fn end(&mut self, instance: u32, _: u32, _: bool) {
        // The machine's emulated time, for the phosphor's decay.
        if let Some(i) = self.vinst.get_mut(&instance) {
            i.frames += 1;
        }
        if let Some(i) = self.tinst.get_mut(&instance) {
            i.frames += 1;
        }
    }

    fn state(&self, instance: u32, slot: u8) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(i) = self.vinst.get(&instance) {
            match slot {
                3 => i.v.write_state(&mut out),
                _ => {
                    if let Some(v3) = &i.v3 {
                        v3.write_state(&mut out);
                    }
                }
            }
        }
        if let Some(i) = self.tinst.get(&instance) {
            i.t.write_state(&mut out);
        }
        out
    }

    fn restore(&mut self, instance: u32, slot: u8, bytes: &[u8]) -> Option<usize> {
        let mut r = terse_core::snapshot::Reader::new(bytes);
        if let Some(i) = self.tinst.get_mut(&instance) {
            let mut t = Terminal::default();
            t.restore(&mut r).ok()?;
            let n = r.position();
            let mut again = Vec::new();
            t.write_state(&mut again);
            (again == bytes[..n]).then_some(())?;
            i.t = t;
            return Some(n);
        }
        let i = self.vinst.get_mut(&instance)?;
        match slot {
            3 => {
                let mut v = Vector::default();
                v.restore(&mut r).ok()?;
                if !i.lists2 && (!v.fill() || !v.list().is_empty()) {
                    return None;
                }
                let n = r.position();
                let mut again = Vec::new();
                v.write_state(&mut again);
                (again == bytes[..n]).then_some(())?;
                i.v = v;
                Some(n)
            }
            _ => {
                let v3 = i.v3.as_mut()?;
                let mut v = Vector3::default();
                v.restore(&mut r).ok()?;
                *v3 = v;
                Some(r.position())
            }
        }
    }

    fn get(&mut self, instance: u32, name: &str) -> Option<Vec<u8>> {
        let i = self.vinst.get(&instance)?;
        match name {
            property::OUTLINES => Some(vec![i.outlines() as u8]),
            property::LIST3 => Some(i.v3.as_ref().map_or(Vec::new(), |v| v.list().to_vec())),
            property::LIST2 => {
                let mut out = Vec::new();
                i.v.write_state(&mut out);
                Some(out)
            }
            TUBE => Some(vec![self.gpu.is_some() as u8]),
            _ => None,
        }
    }

    fn set(&mut self, instance: u32, name: &str, value: &[u8]) {
        let on = value.first().is_some_and(|&v| v != 0);
        if let Some(i) = self.vinst.get_mut(&instance) {
            match name {
                property::EYE if value.len() == 8 => {
                    let f = |k: usize| f32::from_le_bytes(value[k..k + 4].try_into().unwrap());
                    i.eye = (f(0), f(4));
                }
                property::VIEW => i.view = decode_view(value),
                property::CAPS => i.caps = decode_caps(value),
                property::LIGHT => i.light = on,
                property::SHED => i.shed = value.first().copied().unwrap_or(0),
                _ => {}
            }
        }
        if let Some(i) = self.tinst.get_mut(&instance) {
            match name {
                property::STATUS => i.status = String::from_utf8_lossy(value).into_owned(),
                property::LIGHT => i.light = on,
                property::SHED => i.shed = value.first().copied().unwrap_or(0),
                _ => {}
            }
        }
    }

    fn present(&mut self, instance: u32, p: &Present, session: &Session) -> PresentOut {
        if !self.presents {
            return PresentOut::default();
        }
        match self.group {
            Group::Vector => self.present_vector(instance, p, session),
            Group::Terminal => self.present_terminal(instance, p, session),
        }
    }
}

fn main() -> std::process::ExitCode {
    if terse_bus::driver::version(NAME) {
        return std::process::ExitCode::SUCCESS;
    }
    let mut d = Driver {
        group: Group::Vector,
        presents: false,
        slots: Vec::new(),
        gpu: None,
        params: Params::from_env(),
        look: TubeLook::from_env(),
        vinst: HashMap::new(),
        tinst: HashMap::new(),
        times: Times::default(),
    };
    let code = terse_bus::driver::run(&mut d, NAME);
    if std::env::var("TERSE_MAME_TIMES").is_ok_and(|v| !v.is_empty() && v != "0") {
        let adapter = d.gpu.as_ref().map_or("no adapter".to_string(), |g| g.adapter.clone());
        d.times.report(&format!("{NAME} ({})", if d.group == Group::Vector { "vector" } else { "terminal" }), &adapter);
    }
    code
}
