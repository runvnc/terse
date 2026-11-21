//! Device 9, 3D vector display (SPEC §vector3d).
//!
//! The device only validates and copies the display list; the style and
//! persistence it sets are device 3's ([`crate::Vector`]), shared by both
//! displays. [`decode`] turns a stored list into its commands (SPEC §vector3d.list
//! termination rules, the transform stack's limits), and [`project`] turns
//! a list and an eye into the line segments a host draws on the glass, with
//! the caps of SPEC §vector3d.caps, the depth cue of SPEC §vector3d.presentation and what faces hide
//! cut away (SPEC §vector3d.list). Every renderer
//! draws from [`project`], so they all draw the same segments.

use std::fmt;

use crate::vector::{Primitive, Rgba};

/// The device's slot (SPEC §image).
pub const SLOT: u8 = 9;
/// Longest list a submit accepts (SPEC §vector3d.calls).
pub const MAX_LIST: usize = 16_384;
/// Transforms a list can save with Push (SPEC §vector3d.list).
pub const STACK_DEPTH: usize = 8;
/// The reference eye: (320, 240, −960) in screen units (SPEC §vector3d.space).
pub const EYE_X: f64 = 320.0;
pub const EYE_Y: f64 = 240.0;
pub const EYE_DISTANCE: f64 = 960.0;
/// Points with z at or nearer than this (screen units) are not drawn.
pub const NEAR_Z: f64 = -800.0;
/// How far a host may move the eye from the reference eye, on each axis.
pub const MAX_EYE_OFFSET: f32 = 160.0;
/// Total length (screen units, on the glass, reference eye, clipped to the
/// screen) one list may draw (SPEC §vector3d.caps). Provisional.
pub const LENGTH_CAP: f64 = 65_536.0;
/// Segments one list may draw: a line or point is one, a circle
/// [`CIRCLE_CHORDS`] (SPEC §vector3d.caps). Provisional.
pub const SEGMENT_CAP: usize = 16_384;
/// The chords a circle is drawn as.
pub const CIRCLE_CHORDS: usize = 64;
/// The widest line drawn, in sixteenths (that of the 2D display).
pub const MAX_WIDTH: u16 = 255;
/// How far outside the screen (screen units) segments are kept for
/// drawing, so that their ends and glow beyond the edge are drawn as they
/// would be without the cut.
const DRAW_MARGIN: f64 = 32.0;

/// Display-list opcodes (SPEC §vector3d.list).
pub mod op {
    pub const END: u8 = 0x00;
    pub const COLOUR: u8 = 0x01;
    pub const WIDTH: u8 = 0x02;
    pub const MOVE: u8 = 0x03;
    pub const LINE: u8 = 0x04;
    pub const POINT: u8 = 0x05;
    pub const CIRCLE: u8 = 0x06;
    pub const PUSH: u8 = 0x07;
    pub const POP: u8 = 0x08;
    pub const TRANSLATE: u8 = 0x09;
    pub const ROTATE: u8 = 0x0A;
    pub const SCALE: u8 = 0x0B;
    pub const FACE: u8 = 0x0C;
    pub const SCREEN: u8 = 0x0D;
    pub const SURFACE: u8 = 0x0E;
}

/// Surfaces that take effect in one list (SPEC §vector3d.list Surface). Provisional.
pub const MAX_SURFACES: usize = 4;
/// The whole screen as a source rectangle (a Screen's panel shows it all).
pub const WHOLE_SCREEN: [u16; 4] = [0, 0, 640, 480];

/// Bit of [`Primitive::Beam`]'s `butt`: the end continues into the next
/// chord of a circle, so it has no round cap and no vertex dwell.
pub const BUTT_START: u8 = 1;
pub const BUTT_END: u8 = 2;

/// The device state (SPEC §vector3d.determinism): the copy of the list. The style and the
/// persistence are device 3's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Vector3 {
    list: Vec<u8>,
    /// Changes whenever the list's contents change (not machine state).
    generation: u64,
}

impl Vector3 {
    /// Replaces the list with `bytes` if it is 1 to 16,384 bytes long;
    /// returns 0, or 1 (rejected; the list is kept). The caller checks that
    /// the bytes lie in RAM.
    pub fn submit(&mut self, bytes: &[u8]) -> u32 {
        if bytes.is_empty() || bytes.len() > MAX_LIST {
            return crate::vector::RESULT_REJECTED;
        }
        if self.list != bytes {
            self.list.clear();
            self.list.extend_from_slice(bytes);
            self.generation += 1;
        }
        crate::vector::RESULT_DONE
    }

    /// Empties the list.
    pub fn clear(&mut self) {
        if !self.list.is_empty() {
            self.list.clear();
            self.generation += 1;
        }
    }

    pub fn list(&self) -> &[u8] {
        &self.list
    }

    /// A counter that changes whenever [`Vector3::list`] changes.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The list's length and bytes (STATE.md 1.11).
    pub fn write_state(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.list.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.list);
    }

    pub fn restore(&mut self, r: &mut crate::snapshot::Reader) -> Result<(), crate::snapshot::RestoreError> {
        let m = r.u32()? as usize;
        if m > MAX_LIST {
            return Err(crate::snapshot::bad("3D display list length"));
        }
        self.list = r.take(m)?.to_vec();
        self.generation = !self.list.is_empty() as u64;
        Ok(())
    }

    /// The list as `terse run --dump-vectors` prints it: a header, the
    /// commands one per line, and what the reference eye sees.
    pub fn dump(&self) -> String {
        let mut out = format!("-- 3d vectors: {} list bytes --\n", self.list.len());
        for c in decode(&self.list) {
            out.push_str(&format!("{c}\n"));
        }
        let mut segments = Vec::new();
        let p = project(&self.list, (0.0, 0.0), &mut segments);
        let faces = if p.faces > 0 { format!(", {} faces", p.faces) } else { String::new() };
        out.push_str(&format!("-- 3d: {} segments, length {:.1} units{faces}{} --\n", p.segments, p.length, if p.capped { ", capped" } else { "" }));
        // The Surfaces in effect: what they show, and where the reference
        // eye sees their corners on the glass.
        let (panels, _) = surfaces(&self.list, (0.0, 0.0));
        for r in p.surface_rects() {
            out.push_str(&format!("-- surface {} {} {} {}", r[0], r[1], r[2], r[3]));
            match panels.iter().find(|q| q.source == *r) {
                Some(q) => {
                    for c in q.corners {
                        let (x, y) = to_glass(c, (EYE_X, EYE_Y));
                        out.push_str(&format!(" ({x:.1}, {y:.1})"));
                    }
                    out.push_str(if q.front { " front --\n" } else { " back --\n" });
                }
                None => out.push_str(" no area --\n"),
            }
        }
        out
    }
}

/// A point of the list, in sixteenths of a screen unit.
pub type Xyz = [i16; 3];

/// One command of a list (SPEC §vector3d.list), operands as given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Colour(Rgba),
    Width(u16),
    Move(Xyz),
    Line(Xyz),
    Point,
    Circle(u16),
    Push,
    Pop,
    Translate(Xyz),
    /// Axis (0 x, 1 y, 2 z) and angle in 1/65,536 of a turn.
    Rotate(u8, u16),
    /// 8.8 fixed point.
    Scale(u16),
    /// A face: its corners (3 or 4; the fourth is unused for 3).
    Face(u8, [Xyz; 4]),
    /// A machine's screen as a panel: the handle (1-8) and the width in
    /// sixteenths (SPEC §vector3d.list).
    Screen(u8, u16),
    /// A rectangle of the machine's own 2D layers as a panel: x, y, width,
    /// height (screen units, as given) and the panel's width in sixteenths
    /// (SPEC §vector3d.list).
    Surface([u16; 4], u16),
}

impl Command {
    pub fn opcode(&self) -> u8 {
        match self {
            Command::Colour(_) => op::COLOUR,
            Command::Width(_) => op::WIDTH,
            Command::Move(_) => op::MOVE,
            Command::Line(_) => op::LINE,
            Command::Point => op::POINT,
            Command::Circle(_) => op::CIRCLE,
            Command::Push => op::PUSH,
            Command::Pop => op::POP,
            Command::Translate(_) => op::TRANSLATE,
            Command::Rotate(..) => op::ROTATE,
            Command::Scale(_) => op::SCALE,
            Command::Face(..) => op::FACE,
            Command::Screen(..) => op::SCREEN,
            Command::Surface(..) => op::SURFACE,
        }
    }
}

/// A stable one-line text form (`terse run --dump-vectors`).
impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let xyz = |p: &Xyz| format!("{} {} {}", p[0], p[1], p[2]);
        match self {
            Command::Colour(c) => write!(f, "colour {:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3]),
            Command::Width(w) => write!(f, "width {w}"),
            Command::Move(p) => write!(f, "move {}", xyz(p)),
            Command::Line(p) => write!(f, "line {}", xyz(p)),
            Command::Point => write!(f, "point"),
            Command::Circle(r) => write!(f, "circle {r}"),
            Command::Push => write!(f, "push"),
            Command::Pop => write!(f, "pop"),
            Command::Translate(p) => write!(f, "translate {}", xyz(p)),
            Command::Rotate(axis, a) => write!(f, "rotate {} {a}", ["x", "y", "z"][*axis as usize]),
            Command::Scale(s) => write!(f, "scale {s}"),
            Command::Face(n, corners) => {
                write!(f, "face {n}")?;
                corners[..*n as usize].iter().try_for_each(|p| write!(f, " {}", xyz(p)))
            }
            Command::Screen(h, w) => write!(f, "screen {h} {w}"),
            Command::Surface(r, w) => write!(f, "surface {} {} {} {} {w}", r[0], r[1], r[2], r[3]),
        }
    }
}

/// Operand bytes of each opcode (of a Face, from its corner count), or
/// `None` for End, unknown opcodes and a Face whose count is not 3 or 4.
fn operand_len(opcode: u8, first: Option<u8>) -> Option<usize> {
    Some(match opcode {
        op::FACE => match first? {
            n @ (3 | 4) => 1 + 6 * n as usize,
            _ => return None,
        },
        op::COLOUR => 4,
        op::WIDTH | op::CIRCLE | op::SCALE => 2,
        op::MOVE | op::LINE | op::TRANSLATE => 6,
        op::POINT | op::PUSH | op::POP => 0,
        op::ROTATE | op::SCREEN => 3,
        op::SURFACE => 10,
        _ => return None,
    })
}

/// Decodes a list (SPEC §vector3d.list): its commands up to where it ends (an End,
/// the end of the bytes, an unknown opcode, cut-off operands, a Rotate
/// about an axis above 2, a Push with 8 transforms saved or a Pop with none).
pub fn decode(list: &[u8]) -> Decoder<'_> {
    Decoder { list, at: 0, depth: 0 }
}

/// Iterator over the commands of a list; see [`decode`]. A Screen whose
/// handle is not 1-8 ends the list.
#[derive(Clone, Debug)]
pub struct Decoder<'a> {
    list: &'a [u8],
    at: usize,
    depth: usize,
}

impl Iterator for Decoder<'_> {
    type Item = Command;

    fn next(&mut self) -> Option<Command> {
        let &opcode = self.list.get(self.at)?;
        let end = |d: &mut Self| {
            d.at = d.list.len();
            None
        };
        let Some(n) = operand_len(opcode, self.list.get(self.at + 1).copied()) else { return end(self) };
        let Some(o) = self.list.get(self.at + 1..self.at + 1 + n) else { return end(self) };
        let u16_at = |i: usize| u16::from_le_bytes([o[i], o[i + 1]]);
        let xyz = || [u16_at(0) as i16, u16_at(2) as i16, u16_at(4) as i16];
        let c = match opcode {
            op::COLOUR => Command::Colour([o[0], o[1], o[2], o[3]]),
            op::WIDTH => Command::Width(u16_at(0)),
            op::MOVE => Command::Move(xyz()),
            op::LINE => Command::Line(xyz()),
            op::POINT => Command::Point,
            op::CIRCLE => Command::Circle(u16_at(0)),
            op::PUSH => {
                if self.depth == STACK_DEPTH {
                    return end(self);
                }
                self.depth += 1;
                Command::Push
            }
            op::POP => {
                if self.depth == 0 {
                    return end(self);
                }
                self.depth -= 1;
                Command::Pop
            }
            op::TRANSLATE => Command::Translate(xyz()),
            op::ROTATE => {
                if o[0] > 2 {
                    return end(self);
                }
                Command::Rotate(o[0], u16::from_le_bytes([o[1], o[2]]))
            }
            op::SCALE => Command::Scale(u16_at(0)),
            op::SCREEN => {
                if !(1..=8).contains(&o[0]) {
                    return end(self);
                }
                Command::Screen(o[0], u16::from_le_bytes([o[1], o[2]]))
            }
            op::SURFACE => Command::Surface([u16_at(0), u16_at(2), u16_at(4), u16_at(6)], u16_at(8)),
            op::FACE => {
                let at = |i: usize| [0, 2, 4].map(|k| u16::from_le_bytes([o[1 + 6 * i + k], o[2 + 6 * i + k]]) as i16);
                let mut corners = [[0; 3]; 4];
                for (i, c) in corners.iter_mut().enumerate().take(o[0] as usize) {
                    *c = at(i);
                }
                Command::Face(o[0], corners)
            }
            _ => unreachable!(),
        };
        self.at += 1 + n;
        Some(c)
    }
}

/// The opcodes of a list's commands in order: two lists with the same
/// sequence draw the same segments, maybe moved (for the afterglow's smear).
pub fn opcodes(list: &[u8]) -> Vec<u8> {
    decode(list).map(|c| c.opcode()).collect()
}

/// An affine transform of points in screen units: `m` is the linear part
/// (rows), `t` the translation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub m: [[f64; 3]; 3],
    pub t: [f64; 3],
}

impl Transform {
    pub const IDENTITY: Transform = Transform { m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] };

    /// The linear part applied to `v`.
    pub fn linear(&self, v: [f64; 3]) -> [f64; 3] {
        let m = &self.m;
        std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
    }

    pub fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        let v = self.linear(p);
        [v[0] + self.t[0], v[1] + self.t[1], v[2] + self.t[2]]
    }

    /// This transform after `inner`: `self ∘ inner`, applied to a point,
    /// is `inner` first.
    fn then_inner(&self, inner: &Transform) -> Transform {
        let mut m = [[0.0; 3]; 3];
        for (i, row) in m.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = (0..3).map(|k| self.m[i][k] * inner.m[k][j]).sum();
            }
        }
        Transform { m, t: self.apply(inner.t) }
    }

    /// Translate (SPEC §vector3d.list), in screen units.
    pub fn translate(&mut self, v: [f64; 3]) {
        *self = self.then_inner(&Transform { t: v, ..Transform::IDENTITY });
    }

    /// Rotate about `axis` (0 x, 1 y, 2 z) by `turns` of a full turn: a
    /// positive angle turns the axis after `axis` towards the one after
    /// that (about z, x towards y).
    pub fn rotate(&mut self, axis: u8, turns: f64) {
        let (s, c) = (turns * std::f64::consts::TAU).sin_cos();
        let (i, j) = match axis {
            0 => (1, 2),
            1 => (2, 0),
            _ => (0, 1),
        };
        let mut r = Transform::IDENTITY;
        // Axis i goes to (c, s) in the plane of (i, j); axis j to (−s, c).
        r.m[i][i] = c;
        r.m[j][i] = s;
        r.m[i][j] = -s;
        r.m[j][j] = c;
        *self = self.then_inner(&r);
    }

    /// Scale, uniformly, by `s`.
    pub fn scale(&mut self, s: f64) {
        for row in self.m.iter_mut() {
            for v in row.iter_mut() {
                *v *= s;
            }
        }
    }
}

/// Sixteenths to screen units.
fn units(p: Xyz) -> [f64; 3] {
    [p[0] as f64 / 16.0, p[1] as f64 / 16.0, p[2] as f64 / 16.0]
}

/// The list's drawing state at each command (SPEC §vector3d.list): colour, width,
/// the transform and its stack, and the current point in screen units
/// (already transformed: it stays where it was put when the transform
/// changes).
#[derive(Clone, Debug)]
pub struct State {
    pub rgba: Rgba,
    pub width: u16,
    pub transform: Transform,
    pub stack: Vec<Transform>,
    pub point: [f64; 3],
}

impl Default for State {
    fn default() -> Self {
        State { rgba: [255; 4], width: 16, transform: Transform::IDENTITY, stack: Vec::new(), point: [0.0; 3] }
    }
}

/// What a list draws, in screen units: a line between two points, a point,
/// or a circle as its chord polygon (the points at k/64 of a turn); or a
/// face, which hides (its 3 or 4 corners).
#[derive(Clone, Debug, PartialEq)]
pub enum Figure {
    Line([f64; 3], [f64; 3]),
    Point([f64; 3]),
    Circle(Vec<[f64; 3]>),
    Face(Vec<[f64; 3]>),
    /// A machine's screen: the handle and the panel's corners (top left,
    /// top right, bottom right, bottom left, as on the glass).
    Screen(u8, [[f64; 3]; 4]),
    /// A Surface: its source rectangle clamped to the screen, the panel
    /// width (sixteenths) and its corners as a Screen's.
    Surface([u16; 4], u16, [[f64; 3]; 4]),
}

impl State {
    /// Carries out `c`; returns what it draws, if anything.
    pub fn step(&mut self, c: &Command) -> Option<Figure> {
        match *c {
            Command::Colour(rgba) => self.rgba = rgba,
            Command::Width(w) => self.width = w,
            Command::Move(p) => self.point = self.transform.apply(units(p)),
            Command::Line(p) => {
                let to = self.transform.apply(units(p));
                let from = std::mem::replace(&mut self.point, to);
                return Some(Figure::Line(from, to));
            }
            Command::Point => return Some(Figure::Point(self.point)),
            Command::Circle(r) => {
                let r = r as f64 / 16.0;
                let c = self.point;
                let ring = unit_circle()
                    .iter()
                    .map(|&(s, co)| {
                        let v = self.transform.linear([r * co, r * s, 0.0]);
                        [c[0] + v[0], c[1] + v[1], c[2] + v[2]]
                    })
                    .collect();
                return Some(Figure::Circle(ring));
            }
            Command::Push => self.stack.push(self.transform),
            Command::Pop => self.transform = self.stack.pop().unwrap_or(Transform::IDENTITY),
            Command::Translate(p) => self.transform.translate(units(p)),
            Command::Rotate(axis, a) => self.transform.rotate(axis, a as f64 / 65_536.0),
            Command::Scale(s) => self.transform.scale(s as f64 / 256.0),
            Command::Face(n, corners) => return Some(Figure::Face(corners[..n as usize].iter().map(|&p| self.transform.apply(units(p))).collect())),
            Command::Screen(h, w) => {
                let (x, y) = (w as f64 / 32.0, w as f64 * 3.0 / 128.0);
                let corners = [[-x, -y, 0.0], [x, -y, 0.0], [x, y, 0.0], [-x, y, 0.0]].map(|p| self.transform.apply(p));
                return Some(Figure::Screen(h, corners));
            }
            Command::Surface(r, w) => {
                let src = clamp_rect(r);
                // w/16 units wide, as high as the source's shape says.
                let x = w as f64 / 32.0;
                let y = if src[2] == 0 { 0.0 } else { x * src[3] as f64 / src[2] as f64 };
                let corners = [[-x, -y, 0.0], [x, -y, 0.0], [x, y, 0.0], [-x, y, 0.0]].map(|p| self.transform.apply(p));
                return Some(Figure::Surface(src, w, corners));
            }
        }
        None
    }
}

/// A rectangle (x, y, width, height in screen units) clamped so it lies on
/// the screen, as the page's rectangle is (SPEC §page.calls): x to at most 640, y
/// to at most 480, the width to at most 640 − x, the height to at most 480 − y.
pub fn clamp_rect(r: [u16; 4]) -> [u16; 4] {
    let x = r[0].min(640);
    let y = r[1].min(480);
    [x, y, r[2].min(640 - x), r[3].min(480 - y)]
}

/// (sin, cos) of k/64 of a turn, k = 0 to 63: a circle's points.
fn unit_circle() -> &'static [(f64, f64); CIRCLE_CHORDS] {
    static TABLE: std::sync::OnceLock<[(f64, f64); CIRCLE_CHORDS]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| std::array::from_fn(|k| (k as f64 / CIRCLE_CHORDS as f64 * std::f64::consts::TAU).sin_cos()))
}

/// The depth cue at depth `z` (SPEC §vector3d.presentation): 960 / (960 + z), at most 1.
pub fn depth_cue(z: f64) -> f64 {
    (EYE_DISTANCE / (EYE_DISTANCE + z)).min(1.0)
}

/// `p` (in front of the near plane) on the glass, seen from the eye at
/// (ex, ey, −960).
pub fn to_glass(p: [f64; 3], eye: (f64, f64)) -> (f64, f64) {
    let k = EYE_DISTANCE / (EYE_DISTANCE + p[2]);
    (eye.0 + (p[0] - eye.0) * k, eye.1 + (p[1] - eye.1) * k)
}

/// The part of the segment a–b in front of the near plane (z > −800), cut
/// at the plane, or `None`.
pub fn near_clip(a: [f64; 3], b: [f64; 3]) -> Option<([f64; 3], [f64; 3])> {
    let (ina, inb) = (a[2] > NEAR_Z, b[2] > NEAR_Z);
    let cut = |p: [f64; 3], q: [f64; 3]| {
        let t = (NEAR_Z - p[2]) / (q[2] - p[2]);
        [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t, NEAR_Z]
    };
    match (ina, inb) {
        (true, true) => Some((a, b)),
        (false, false) => None,
        (true, false) => Some((a, cut(a, b))),
        (false, true) => Some((cut(b, a), b)),
    }
}

/// The parameters (t0, t1) of the part of the segment a–b (on the glass)
/// inside the rectangle (x0, y0)–(x1, y1), or `None` (Liang–Barsky).
pub fn clip_rect(a: (f64, f64), b: (f64, f64), r: (f64, f64, f64, f64)) -> Option<(f64, f64)> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-dx, a.0 - r.0), (dx, r.2 - a.0), (-dy, a.1 - r.1), (dy, r.3 - a.1)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
        }
    }
    (t0 <= t1).then_some((t0, t1))
}

/// The screen (SPEC §screen), in screen units.
const SCREEN: (f64, f64, f64, f64) = (0.0, 0.0, 640.0, 480.0);

/// The length on the glass, seen from the reference eye, of the part of
/// a–b in front of the near plane and on the screen (SPEC §vector3d.caps).
pub fn counted_length(a: [f64; 3], b: [f64; 3]) -> f64 {
    let Some((a, b)) = near_clip(a, b) else { return 0.0 };
    let reference = (EYE_X, EYE_Y);
    let (p, q) = (to_glass(a, reference), to_glass(b, reference));
    match clip_rect(p, q, SCREEN) {
        Some((t0, t1)) => (t1 - t0) * ((q.0 - p.0).powi(2) + (q.1 - p.1).powi(2)).sqrt(),
        None => 0.0,
    }
}

/// The ink of the segment a–b drawn `width` sixteenths wide (SPEC
/// §vector3d.caps), in square screen units: its part in front of the near
/// plane seen on the glass from the reference eye, counted as a 2D line
/// is (SPEC §vector.ink-cap): (|dx| + |dy| + w) × (w + 3) with w the
/// width in screen units, or the box it can reach on the screen if that
/// is smaller. A point is a segment of length 0.
pub fn segment_ink(a: [f64; 3], b: [f64; 3], width: u16) -> f64 {
    let Some((a, b)) = near_clip(a, b) else { return 0.0 };
    let reference = (EYE_X, EYE_Y);
    let (p, q) = (to_glass(a, reference), to_glass(b, reference));
    let w = width.clamp(1, MAX_WIDTH) as f64 / 16.0;
    let stroke = ((q.0 - p.0).abs() + (q.1 - p.1).abs() + w) * (w + 3.0);
    let g = (w + 3.0) / 2.0;
    let bw = (p.0.max(q.0) + g).min(SCREEN.2) - (p.0.min(q.0) - g).max(SCREEN.0);
    let bh = (p.1.max(q.1) + g).min(SCREEN.3) - (p.1.min(q.1) - g).max(SCREEN.1);
    stroke.min(bw.max(0.0) * bh.max(0.0))
}

/// The ink of a panel (a Screen or Surface in effect) with these corners
/// (SPEC §vector3d.caps): the area of the box its corners make on the
/// glass, seen from the reference eye, clipped to the screen; the whole
/// screen if a corner is not in front of the near plane.
pub fn panel_ink(corners: &[[f64; 3]; 4]) -> f64 {
    if corners.iter().any(|c| c[2] <= NEAR_Z) {
        return SCREEN.2 * SCREEN.3;
    }
    let glass = corners.map(|c| to_glass(c, (EYE_X, EYE_Y)));
    let (x0, x1) = glass.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.0), b.max(p.0)));
    let (y0, y1) = glass.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.1), b.max(p.1)));
    let w = (x1.min(SCREEN.2) - x0.max(SCREEN.0)).max(0.0);
    let h = (y1.min(SCREEN.3) - y0.max(SCREEN.1)).max(0.0);
    w * h
}

/// Most ink one 3D list may draw (SPEC §vector3d.caps), in square screen
/// units.
pub const INK_CAP: f64 = INK_CAP_SCREENS * 640.0 * 480.0;
/// [`INK_CAP`] in screens.
pub const INK_CAP_SCREENS: f64 = 0.5;

/// Faces one list may give (SPEC §vector3d.caps).
pub const FACE_CAP: usize = 256;
/// The most faces × segments one list may give (SPEC §vector3d.caps): it bounds the
/// work of cutting the segments against the faces.
pub const FACE_WORK_CAP: usize = 131_072;
/// How far beyond a face's plane (screen units, measured perpendicular to
/// it, away from the eye) a point must lie for the face to hide it: 2
/// sixteenths. A line in a face's plane or along its edge is not hidden by it.
pub const HIDE_TOLERANCE: f64 = 0.125;
/// A visible piece with an end where it was cut is not drawn if it is
/// shorter than this on the glass (screen units) and the segment it is a
/// piece of is not.
pub const MIN_PIECE: f64 = 0.5;

/// What a list drew.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Projection {
    /// Segments drawn (`out`'s length): after cutting, from the eye used.
    pub segments: usize,
    /// Their counted length (SPEC §vector3d.caps), in screen units, as if there were
    /// no faces.
    pub length: f64,
    /// Their ink (SPEC §vector3d.caps), in square screen units, as if there
    /// were no faces.
    pub ink: f64,
    /// Segments as the caps count them (a circle 64), before cutting.
    pub counted: usize,
    /// Faces in effect.
    pub faces: usize,
    /// A command was stopped by a cap (or a segment by the cap on what is
    /// drawn after cutting).
    pub capped: bool,
    /// Surfaces in effect (SPEC §vector3d.list), and their source rectangles (clamped)
    /// in list order: what is not drawn on the glass.
    pub surfaces: usize,
    pub rects: [[u16; 4]; MAX_SURFACES],
}

impl Projection {
    /// The source rectangles of the Surfaces in effect.
    pub fn surface_rects(&self) -> &[[u16; 4]] {
        &self.rects[..self.surfaces]
    }
}

/// The segments a host draws for `list` (SPEC §vector3d.caps, §vector3d.presentation), with the eye
/// moved `eye` (screen units, each at most [`MAX_EYE_OFFSET`]) from the
/// reference eye, as [`Primitive::Beam`]s in sixteenths on the glass, in
/// list order, appended to `out`. Lines and points have round ends; a
/// circle's chords join with butt ends (`butt`), and so do the ends where
/// a face cut a segment. Each end carries its depth cue (`fade`, 255 = 1).
/// Segments are clipped to the near plane and to the screen grown by a
/// margin, and what the list's faces hide from this eye is cut away. What
/// the caps count is measured with the reference eye and without faces,
/// whatever `eye` is.
pub fn project(list: &[u8], eye: (f32, f32), out: &mut Vec<Primitive>) -> Projection {
    project_pieces(list, eye, out, &mut Vec::new())
}

/// [`project`], and into `pieces` (cleared), when the list has faces, the
/// number of segments each line, point and circle gave, as 2 bytes
/// (little-endian) each: two projections with the same opcodes and the
/// same `pieces` give segments that line up one for one. Screens are
/// ignored (the list of a machine that is not the shell, SPEC §vector3d.list);
/// Surfaces in effect hide as faces do (their panels are not returned: see
/// [`project_scene`]).
pub fn project_pieces(list: &[u8], eye: (f32, f32), out: &mut Vec<Primitive>, pieces: &mut Vec<u8>) -> Projection {
    project_scene(list, eye, &SceneView::LISTED, out, pieces, &mut Vec::new())
}

/// How the shell's scene is shown (SPEC §machines.screens, §vector3d.list): which handles' Screens
/// are in effect (bit h − 1 for handle h) and the host's view of the whole
/// scene (`view`, SPEC §machines.calls `face`): a rigid motion and uniform scale
/// applied to everything after the caps are counted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneView {
    pub screens: u8,
    pub view: Transform,
}

impl Default for SceneView {
    fn default() -> Self {
        SceneView::LISTED
    }
}

impl SceneView {
    /// No Screens, the scene as listed.
    pub const LISTED: SceneView = SceneView { screens: 0, view: Transform::IDENTITY };
}

/// A Screen or a Surface in effect as the eye sees it: the machine whose
/// screen it shows (its handle; 0 for a Surface, which shows the list's own
/// machine), the rectangle of that screen it shows (screen units: the whole
/// screen for a Screen), its corners (top left, top right, bottom right,
/// bottom left of that rectangle) in screen units after the view, whether
/// its front is towards the eye, and the distance from the eye to its
/// centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Panel {
    pub handle: u8,
    pub source: [u16; 4],
    pub corners: [[f64; 3]; 4],
    pub front: bool,
    pub distance: f64,
}

impl Panel {
    /// Whether it is a Surface (the list's own machine's 2D layers).
    pub fn is_surface(&self) -> bool {
        self.handle == 0
    }
}

/// What a list gives once its caps are applied (SPEC §vector3d.caps): the figures
/// that are drawn (with their width and colour), the faces (the Screens and
/// Surfaces in effect among them) and the counts.
struct Walk {
    drawn: Vec<(Figure, u8, Rgba)>,
    faces: Vec<Vec<[f64; 3]>>,
    projection: Projection,
}

/// Walks `list` as [`project_scene`] does, without cutting any segment:
/// what the caps let through, and the panels into `panels` (cleared), far
/// to near.
fn walk(list: &[u8], eye: (f64, f64), scene: &SceneView, panels: &mut Vec<Panel>) -> Walk {
    walk_capped(list, eye, scene, panels, INK_CAP)
}

/// What a list's caps count (SPEC §vector3d.caps) with the ink cap left
/// out (the other caps applied as usual): what the list asks of a host.
/// `screens` are the handles whose Screens are in effect (as in
/// [`SceneView`]).
pub fn counts(list: &[u8], screens: u8) -> Projection {
    let scene = SceneView { screens, view: Transform::IDENTITY };
    walk_capped(list, eye_at((0.0, 0.0)), &scene, &mut Vec::new(), f64::INFINITY).projection
}

fn walk_capped(list: &[u8], eye: (f64, f64), scene: &SceneView, panels: &mut Vec<Panel>, ink_cap: f64) -> Walk {
    panels.clear();
    let mut state = State::default();
    let (mut length, mut count, mut ink) = (0.0f64, 0usize, 0.0f64);
    let mut projection = Projection::default();
    let mut drawn = Vec::new();
    let mut faces = Vec::new();
    let mut seen = 0u8;
    let v = &scene.view;
    let moved = *v != Transform::IDENTITY;
    for c in decode(list) {
        let Some(figure) = state.step(&c) else { continue };
        match figure {
            Figure::Screen(h, _) => {
                // Only the first Screen of a handle, and only in effect.
                let first = seen & 1 << (h - 1) == 0;
                seen |= 1 << (h - 1);
                if !first || scene.screens & 1 << (h - 1) == 0 {
                    continue;
                }
            }
            // The first four with a source rectangle and a width.
            Figure::Surface(src, w, _) if src[2] == 0 || src[3] == 0 || w == 0 || projection.surfaces == MAX_SURFACES => continue,
            _ => {}
        }
        let w = state.width;
        let (n, len, f, k) = match &figure {
            Figure::Line(a, b) => (1, counted_length(*a, *b), 0, segment_ink(*a, *b, w)),
            Figure::Point(p) => (1, 0.0, 0, segment_ink(*p, *p, w)),
            Figure::Circle(ring) => {
                let chord = |k: usize| (ring[k], ring[(k + 1) % ring.len()]);
                let len = (0..ring.len()).map(|k| counted_length(chord(k).0, chord(k).1)).sum();
                (CIRCLE_CHORDS, len, 0, (0..ring.len()).map(|k| segment_ink(chord(k).0, chord(k).1, w)).sum())
            }
            Figure::Face(_) | Figure::Screen(..) => (0, 0.0, 1, 0.0),
            Figure::Surface(_, _, corners) => (0, 0.0, 1, panel_ink(corners)),
        };
        let nf = faces.len() + f;
        if length + len > LENGTH_CAP
            || count + n > SEGMENT_CAP
            || nf > FACE_CAP
            || nf * (count + n) > FACE_WORK_CAP
            || ink + k > ink_cap
        {
            projection.capped = true;
            break;
        }
        length += len;
        count += n;
        ink += k;
        let figure = if moved { figure.moved(v) } else { figure };
        match figure {
            Figure::Face(corners) => faces.push(corners),
            Figure::Screen(h, corners) => {
                faces.push(corners.to_vec());
                if let Some(p) = Panel::seen(h, WHOLE_SCREEN, corners, eye) {
                    panels.push(p);
                }
            }
            Figure::Surface(src, _, corners) => {
                faces.push(corners.to_vec());
                projection.rects[projection.surfaces] = src;
                projection.surfaces += 1;
                if let Some(p) = Panel::seen(0, src, corners, eye) {
                    panels.push(p);
                }
            }
            figure => drawn.push((figure, state.width.clamp(1, MAX_WIDTH) as u8, state.rgba)),
        }
    }
    // Far to near; the lower handle first where they are as far (a
    // Surface, handle 0, before a Screen), then in list order.
    panels.sort_by(|a, b| b.distance.total_cmp(&a.distance).then(a.handle.cmp(&b.handle)));
    projection.length = length;
    projection.ink = ink;
    projection.counted = count;
    projection.faces = faces.len();
    Walk { drawn, faces, projection }
}

/// The Surfaces in effect in a machine's own `list` (SPEC §vector3d.list), as the
/// eye moved `eye` from the reference eye sees them, far to near, and what
/// the caps let through (no segments are cut: `segments` is 0).
pub fn surfaces(list: &[u8], eye: (f32, f32)) -> (Vec<Panel>, Projection) {
    let mut panels = Vec::new();
    let w = walk(list, eye_at(eye), &SceneView::LISTED, &mut panels);
    (panels, w.projection)
}

/// Whether `list` has a Surface command (in effect or not).
pub fn has_surface(list: &[u8]) -> bool {
    decode(list).any(|c| matches!(c, Command::Surface(..)))
}

/// Where a pointer at (x, y) on the glass goes on the machine whose list is
/// `list` (SPEC §vector3d.list Surface): over the front of a Surface in effect (the
/// nearest one the ray from the eye meets), the point of its source
/// rectangle; otherwise (x, y) itself.
pub fn surface_pointer(list: &[u8], eye: (f32, f32), x: u16, y: u16) -> (u16, u16) {
    if !has_surface(list) {
        return (x, y);
    }
    let (panels, _) = surfaces(list, eye);
    match panel_hit(&panels, eye, x as f64 + 0.5, y as f64 + 0.5) {
        Some((_, Some(at))) => at,
        _ => (x, y),
    }
}

/// The eye moved `eye` (each at most [`MAX_EYE_OFFSET`]) from the reference
/// eye, in screen units.
fn eye_at(eye: (f32, f32)) -> (f64, f64) {
    let clamp = |v: f32| v.clamp(-MAX_EYE_OFFSET, MAX_EYE_OFFSET) as f64;
    (EYE_X + clamp(eye.0), EYE_Y + clamp(eye.1))
}

/// [`project_pieces`] for a list with panels (SPEC §vector3d.list Screen, Surface):
/// the Screens of the handles in `scene.screens` (the first of each handle;
/// only the shell's list has any in effect) and the first four Surfaces
/// count as faces and hide what is behind them, and go into `panels`
/// (cleared), far to near; everything is moved by `scene.view` before it is
/// seen from the eye. What the caps count is measured on the list as given.
pub fn project_scene(
    list: &[u8],
    eye: (f32, f32),
    scene: &SceneView,
    out: &mut Vec<Primitive>,
    pieces: &mut Vec<u8>,
    panels: &mut Vec<Panel>,
) -> Projection {
    let eye = eye_at(eye);
    let start = out.len();
    pieces.clear();
    let Walk { drawn, faces, mut projection } = walk(list, eye, scene, panels);
    let mut hider = Hider::new(&faces, eye);
    let mut s = Segments { eye, hider: &mut hider, out, room: start + SEGMENT_CAP, capped: false };
    for (figure, width, rgba) in drawn {
        let before = s.out.len();
        match figure {
            Figure::Line(a, b) => s.segment(a, b, 0, width, rgba),
            Figure::Point(p) => s.segment(p, p, 0, width, rgba),
            Figure::Circle(ring) => {
                for k in 0..ring.len() {
                    s.segment(ring[k], ring[(k + 1) % ring.len()], BUTT_START | BUTT_END, width, rgba);
                }
            }
            Figure::Face(_) | Figure::Screen(..) | Figure::Surface(..) => unreachable!(),
        }
        if !faces.is_empty() {
            pieces.extend_from_slice(&((s.out.len() - before).min(u16::MAX as usize) as u16).to_le_bytes());
        }
    }
    projection.capped |= s.capped;
    projection.segments = out.len() - start;
    projection
}

impl Figure {
    /// The figure with every point moved by `t`.
    fn moved(self, t: &Transform) -> Figure {
        match self {
            Figure::Line(a, b) => Figure::Line(t.apply(a), t.apply(b)),
            Figure::Point(p) => Figure::Point(t.apply(p)),
            Figure::Circle(ring) => Figure::Circle(ring.into_iter().map(|p| t.apply(p)).collect()),
            Figure::Face(c) => Figure::Face(c.into_iter().map(|p| t.apply(p)).collect()),
            Figure::Screen(h, c) => Figure::Screen(h, c.map(|p| t.apply(p))),
            Figure::Surface(r, w, c) => Figure::Surface(r, w, c.map(|p| t.apply(p))),
        }
    }
}

impl Panel {
    /// The panel of handle `h` (0: a Surface) showing `source`, with these
    /// corners, seen from the eye at (ex, ey, −960); `None` if it has no
    /// area.
    fn seen(handle: u8, source: [u16; 4], corners: [[f64; 3]; 4], eye: (f64, f64)) -> Option<Panel> {
        let e = [eye.0, eye.1, -EYE_DISTANCE];
        let n = cross(sub(corners[1], corners[0]), sub(corners[3], corners[0]));
        if dot(n, n) < 1e-18 {
            return None;
        }
        let c = centre(&corners);
        let to_eye = sub(e, c);
        Some(Panel { handle, source, corners, front: dot(n, to_eye) < 0.0, distance: dot(to_eye, to_eye).sqrt() })
    }
}

/// The centre of a panel's four corners.
pub fn centre(c: &[[f64; 3]; 4]) -> [f64; 3] {
    std::array::from_fn(|i| (c[0][i] + c[1][i] + c[2][i] + c[3][i]) / 4.0)
}

/// The handles that have a Screen in `list` as it decodes (SPEC §vector3d.list), bit
/// h − 1 for handle h, whatever the caps.
pub fn screens_in(list: &[u8]) -> u8 {
    decode(list).fold(0, |m, c| if let Command::Screen(h, _) = c { m | 1 << (h - 1) } else { m })
}

/// The corners of the first Screen of handle `h` in `list`, as listed
/// (no view), if it has one.
pub fn screen_corners(list: &[u8], h: u8) -> Option<[[f64; 3]; 4]> {
    let mut state = State::default();
    for c in decode(list) {
        if let Some(Figure::Screen(k, corners)) = state.step(&c)
            && k == h
        {
            return Some(corners);
        }
    }
    None
}

/// The pointer at (x, y) on the glass (screen units) seen from the eye at
/// `eye` (moved from the reference eye, as for [`project`]) against the
/// panels (SPEC §machines.input, §vector3d.list Surface): the nearest panel the ray from the eye
/// through the point meets in front of the near plane, its handle (0 for a
/// Surface), and where on its machine's screen it is (in its source
/// rectangle; 0-639, 0-479 for a Screen), or `None` there if it is seen
/// from behind.
pub fn panel_hit(panels: &[Panel], eye: (f32, f32), x: f64, y: f64) -> Option<(u8, Option<(u16, u16)>)> {
    let eye = eye_at(eye);
    let e = [eye.0, eye.1, -EYE_DISTANCE];
    let d = sub([x, y, 0.0], e);
    let mut best: Option<(f64, u8, Option<(u16, u16)>)> = None;
    for p in panels {
        let c = &p.corners;
        let (ex, ey) = (sub(c[1], c[0]), sub(c[3], c[0]));
        let n = cross(ex, ey);
        let dn = dot(n, d);
        if dn.abs() < 1e-12 {
            continue; // edge on
        }
        let t = dot(n, sub(c[0], e)) / dn;
        let q = [e[0] + d[0] * t, e[1] + d[1] * t, e[2] + d[2] * t];
        if t <= 0.0 || q[2] <= NEAR_Z || best.is_some_and(|b| b.0 <= t) {
            continue;
        }
        let r = sub(q, c[0]);
        let (u, v) = (dot(r, ex) / dot(ex, ex), dot(r, ey) / dot(ey, ey));
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            continue;
        }
        let [sx, sy, sw, sh] = p.source;
        let at = p.front.then(|| (sx + ((u * sw as f64) as u16).min(sw - 1), sy + ((v * sh as f64) as u16).min(sh - 1)));
        best = Some((t, p.handle, at));
    }
    best.map(|(_, h, at)| (h, at))
}

/// The view of the whole scene in which a panel with corners `c` (as
/// listed) has moved the fraction `e` (0 to 1, eased) of the way to lying
/// flat on the glass over `target` (x, y, width, height in screen units):
/// its centre goes straight to the target's, it turns by the shorter way
/// (a slerp) to face the viewer upright, and its width changes
/// geometrically to the target's width (SPEC §machines.calls `face`).
pub fn view_transform(c: &[[f64; 3]; 4], target: (f64, f64, f64, f64), e: f64) -> Transform {
    let len = |v: [f64; 3]| dot(v, v).sqrt();
    let (ax, ay) = (sub(c[1], c[0]), sub(c[3], c[0]));
    let w0 = len(ax);
    if w0 < 1e-9 || len(ay) < 1e-9 {
        return Transform::IDENTITY;
    }
    let ux = ax.map(|v| v / w0);
    let uy = ay.map(|v| v / len(ay));
    let uz = cross(ux, uy);
    // The panel's orientation: its axes are the columns.
    let r0 = [[ux[0], uy[0], uz[0]], [ux[1], uy[1], uz[1]], [ux[2], uy[2], uz[2]]];
    let q0 = quaternion(&r0);
    // Towards the identity, the shorter way.
    let q = slerp(q0, [1.0, 0.0, 0.0, 0.0], e);
    let r = rotation(q);
    let c0 = centre(c);
    let c1 = [target.0 + target.2 / 2.0, target.1 + target.3 / 2.0, 0.0];
    let ce = [c0[0] + (c1[0] - c0[0]) * e, c0[1] + (c1[1] - c0[1]) * e, c0[2] + (c1[2] - c0[2]) * e];
    let s = (target.2 / w0).powf(e);
    // V(p) = ce + s R R0ᵀ (p − c0).
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = s * (0..3).map(|k| r[i][k] * r0[j][k]).sum::<f64>();
        }
    }
    let lin = Transform { m, t: [0.0; 3] };
    let mc = lin.linear(c0);
    Transform { m, t: [ce[0] - mc[0], ce[1] - mc[1], ce[2] - mc[2]] }
}

/// The eased fraction of a view's motion (SPEC §machines.calls `face`): smoothstep of
/// the linear progress `q` (0 to 4096).
pub fn ease(q: u16) -> f64 {
    let t = (q.min(4096) as f64) / 4096.0;
    t * t * (3.0 - 2.0 * t)
}

/// The unit quaternion (w, x, y, z) of a rotation matrix.
fn quaternion(m: &[[f64; 3]; 3]) -> [f64; 4] {
    let tr = m[0][0] + m[1][1] + m[2][2];
    let q = if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        [0.25 * s, (m[2][1] - m[1][2]) / s, (m[0][2] - m[2][0]) / s, (m[1][0] - m[0][1]) / s]
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        [(m[2][1] - m[1][2]) / s, 0.25 * s, (m[0][1] + m[1][0]) / s, (m[0][2] + m[2][0]) / s]
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        [(m[0][2] - m[2][0]) / s, (m[0][1] + m[1][0]) / s, 0.25 * s, (m[1][2] + m[2][1]) / s]
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        [(m[1][0] - m[0][1]) / s, (m[0][2] + m[2][0]) / s, (m[1][2] + m[2][1]) / s, 0.25 * s]
    };
    let n = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    q.map(|v| v / n)
}

/// From `a` to `b` by the fraction `t`, the shorter way.
fn slerp(a: [f64; 4], mut b: [f64; 4], t: f64) -> [f64; 4] {
    let mut d: f64 = (0..4).map(|i| a[i] * b[i]).sum();
    if d < 0.0 {
        b = b.map(|v| -v);
        d = -d;
    }
    let (ka, kb) = if d > 0.9995 {
        (1.0 - t, t)
    } else {
        let th = d.min(1.0).acos();
        let s = th.sin();
        (((1.0 - t) * th).sin() / s, (t * th).sin() / s)
    };
    let q: [f64; 4] = std::array::from_fn(|i| ka * a[i] + kb * b[i]);
    let n = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    q.map(|v| v / n)
}

/// The rotation matrix of a unit quaternion.
fn rotation(q: [f64; 4]) -> [[f64; 3]; 3] {
    let [w, x, y, z] = q;
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)],
        [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x)],
        [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

/// Where segments go: the eye, the faces, the output and how far it may
/// grow.
struct Segments<'a> {
    eye: (f64, f64),
    hider: &'a mut Hider,
    out: &'a mut Vec<Primitive>,
    room: usize,
    capped: bool,
}

impl Segments<'_> {
    /// The segment a–b (screen units) seen from the eye: the pieces of it
    /// that are drawn, if any.
    fn segment(&mut self, a: [f64; 3], b: [f64; 3], butt: u8, width: u8, rgba: Rgba) {
        let Some((a, b)) = near_clip(a, b) else { return };
        let (p, q) = (to_glass(a, self.eye), to_glass(b, self.eye));
        let m = DRAW_MARGIN;
        let Some((t0, t1)) = clip_rect(p, q, (-m, -m, 640.0 + m, 480.0 + m)) else { return };
        // The cue is linear on the glass along a projected line (it is the
        // projection's own scale).
        let (f0, f1) = (depth_cue(a[2]), depth_cue(b[2]));
        let at = |t: f64| {
            let x = (p.0 + (q.0 - p.0) * t) * 16.0;
            let y = (p.1 + (q.1 - p.1) * t) * 16.0;
            let f = f0 + (f1 - f0) * t;
            (x.round() as i16, y.round() as i16, (f * 255.0).round() as u8)
        };
        let glass = ((q.0 - p.0).powi(2) + (q.1 - p.1).powi(2)).sqrt();
        let Segments { hider, out, room, capped, .. } = self;
        let hidden = hider.cut(a, b, p, q, (t0, t1));
        // The visible pieces: between the hidden parts; an end where a
        // piece was cut is a butt end.
        let mut piece = |u0: f64, u1: f64, cut0: bool, cut1: bool| {
            if (cut0 || cut1) && glass >= MIN_PIECE && (u1 - u0) * glass < MIN_PIECE {
                return;
            }
            if out.len() >= *room {
                *capped = true;
                return;
            }
            let ((x0, y0, g0), (x1, y1, g1)) = (at(u0), at(u1));
            let butt = butt | if cut0 { BUTT_START } else { 0 } | if cut1 { BUTT_END } else { 0 };
            out.push(Primitive::Beam { x0, y0, x1, y1, width, rgba, fade: [g0, g1], butt });
        };
        if hidden.is_empty() {
            piece(t0, t1, false, false);
            return;
        }
        let (mut from, mut cut) = (t0, false);
        for &(h0, h1) in hidden {
            if h0 > from {
                piece(from, h0, cut, true);
            }
            (from, cut) = (h1, true);
        }
        if t1 > from {
            piece(from, t1, true, false);
        }
    }
}

/// A face's triangle as seen from the eye: its edges on the glass, its
/// plane, how near it comes.
#[derive(Clone, Copy, Debug)]
struct Tri {
    /// (a, b, c): a x + b y + c ≥ 0 inside the triangle on the glass.
    edges: [[f64; 3]; 3],
    /// x0, y0, x1, y1 on the glass.
    bbox: [f64; 4],
    /// Unit normal and offset: n·X − offset is the distance from the
    /// plane, positive on the eye's side.
    normal: [f64; 3],
    offset: f64,
    /// The least z of its corners (in front of the near plane).
    near: f64,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// The faces of a list as triangles seen from one eye, nearest first, and
/// the hidden parts of a segment.
struct Hider {
    tris: Vec<Tri>,
    hidden: Vec<(f64, f64)>,
}

impl Hider {
    fn new(faces: &[Vec<[f64; 3]>], eye: (f64, f64)) -> Hider {
        let e = [eye.0, eye.1, -EYE_DISTANCE];
        let m = DRAW_MARGIN;
        let area = [-m, -m, 640.0 + m, 480.0 + m];
        let mut tris = Vec::new();
        for f in faces {
            let halves: &[[usize; 3]] = if f.len() == 4 { &[[0, 1, 2], [0, 2, 3]] } else { &[[0, 1, 2]] };
            for h in halves {
                let c = h.map(|i| f[i]);
                let n = cross(sub(c[1], c[0]), sub(c[2], c[0]));
                let l = dot(n, n).sqrt();
                if l < 1e-9 {
                    continue; // no area: hides nothing
                }
                let mut normal = n.map(|v| v / l);
                let mut offset = dot(normal, c[0]);
                let de = dot(normal, e) - offset;
                if de.abs() < 1e-9 {
                    continue; // seen edge on
                }
                if de < 0.0 {
                    normal = normal.map(|v| -v);
                    offset = -offset;
                }
                // The part in front of the near plane, as a fan.
                let mut poly: Vec<[f64; 3]> = Vec::with_capacity(4);
                for k in 0..3 {
                    let (p, q) = (c[k], c[(k + 1) % 3]);
                    if p[2] > NEAR_Z {
                        poly.push(p);
                    }
                    if (p[2] > NEAR_Z) != (q[2] > NEAR_Z) {
                        let t = (NEAR_Z - p[2]) / (q[2] - p[2]);
                        poly.push([p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t, NEAR_Z]);
                    }
                }
                for k in 1..poly.len().saturating_sub(1) {
                    let corners = [poly[0], poly[k], poly[k + 1]];
                    let g = corners.map(|p| to_glass(p, eye));
                    let a = (g[1].0 - g[0].0) * (g[2].1 - g[0].1) - (g[1].1 - g[0].1) * (g[2].0 - g[0].0);
                    if a.abs() < 1e-9 {
                        continue;
                    }
                    let sign = a.signum();
                    let edges = std::array::from_fn(|i| {
                        let (p, q) = (g[i], g[(i + 1) % 3]);
                        let (ea, eb) = (-sign * (q.1 - p.1), sign * (q.0 - p.0));
                        [ea, eb, -(ea * p.0 + eb * p.1)]
                    });
                    let bbox = [
                        g.iter().map(|p| p.0).fold(f64::INFINITY, f64::min),
                        g.iter().map(|p| p.1).fold(f64::INFINITY, f64::min),
                        g.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max),
                        g.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max),
                    ];
                    if bbox[0] > area[2] || bbox[2] < area[0] || bbox[1] > area[3] || bbox[3] < area[1] {
                        continue;
                    }
                    let near = corners.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min);
                    tris.push(Tri { edges, bbox, normal, offset, near });
                }
            }
        }
        tris.sort_by(|a, b| a.near.total_cmp(&b.near));
        Hider { tris, hidden: Vec::new() }
    }

    /// The hidden parts of the segment a–b (in front of the near plane),
    /// seen on the glass from p to q, within the parameters `range` along
    /// p–q: sorted, disjoint parameter intervals.
    fn cut(&mut self, a: [f64; 3], b: [f64; 3], p: (f64, f64), q: (f64, f64), range: (f64, f64)) -> &[(f64, f64)] {
        let hidden = &mut self.hidden;
        hidden.clear();
        // Only a face nearer than the segment's far end can hide any of it.
        let far = a[2].max(b[2]);
        let n = self.tris.partition_point(|t| t.near < far);
        if n == 0 {
            return hidden;
        }
        let (t0, t1) = range;
        let (dx, dy) = (q.0 - p.0, q.1 - p.1);
        let (s0, s1) = ((p.0 + dx * t0, p.1 + dy * t0), (p.0 + dx * t1, p.1 + dy * t1));
        let bbox = [s0.0.min(s1.0), s0.1.min(s1.1), s0.0.max(s1.0), s0.1.max(s1.1)];
        let (wa, wb) = (EYE_DISTANCE + a[2], EYE_DISTANCE + b[2]);
        for t in &self.tris[..n] {
            if t.bbox[0] > bbox[2] || t.bbox[2] < bbox[0] || t.bbox[1] > bbox[3] || t.bbox[3] < bbox[1] {
                continue;
            }
            // Where p–q is inside the triangle on the glass: nowhere if both
            // ends are outside one edge; else cut at the edges it crosses.
            let f = t.edges.map(|e| (e[0] * p.0 + e[1] * p.1 + e[2], e[0] * q.0 + e[1] * q.1 + e[2]));
            if f.iter().any(|&(fp, fq)| fp < 0.0 && fq < 0.0) {
                continue;
            }
            let (mut lo, mut hi) = (t0, t1);
            for (fp, fq) in f {
                if fp < 0.0 {
                    lo = lo.max(fp / (fp - fq));
                } else if fq < 0.0 {
                    hi = hi.min(fp / (fp - fq));
                }
            }
            if lo >= hi {
                continue;
            }
            // Where the segment lies beyond the plane by more than the
            // tolerance: h / w is linear on the glass, and w > 0.
            let ha = (t.offset - dot(t.normal, a) - HIDE_TOLERANCE) / wa;
            let hb = (t.offset - dot(t.normal, b) - HIDE_TOLERANCE) / wb;
            if ha <= 0.0 && hb <= 0.0 {
                continue;
            }
            if ha <= 0.0 {
                lo = lo.max(ha / (ha - hb));
            } else if hb <= 0.0 {
                hi = hi.min(ha / (ha - hb));
            }
            if lo >= hi {
                continue;
            }
            if lo <= t0 && hi >= t1 {
                hidden.clear();
                hidden.push((t0, t1));
                return hidden;
            }
            hidden.push((lo, hi));
        }
        hidden.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut merged = 0;
        for i in 0..hidden.len() {
            let h = hidden[i];
            if merged > 0 && h.0 <= hidden[merged - 1].1 {
                hidden[merged - 1].1 = hidden[merged - 1].1.max(h.1);
            } else {
                hidden[merged] = h;
                merged += 1;
            }
        }
        hidden.truncate(merged);
        hidden
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xyz(x: i16, y: i16, z: i16) -> Vec<u8> {
        [x.to_le_bytes(), y.to_le_bytes(), z.to_le_bytes()].concat()
    }

    fn cmd(opcode: u8, operands: &[u8]) -> Vec<u8> {
        [&[opcode][..], operands].concat()
    }

    fn all(list: &[u8]) -> Vec<Command> {
        decode(list).collect()
    }

    fn beams(list: &[u8], eye: (f32, f32)) -> Vec<Primitive> {
        let mut out = Vec::new();
        project(list, eye, &mut out);
        out
    }

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9)
    }

    #[test]
    fn every_opcode() {
        let list = [
            cmd(op::COLOUR, &[1, 2, 3, 4]),
            cmd(op::WIDTH, &300u16.to_le_bytes()),
            cmd(op::MOVE, &xyz(1, -2, 3)),
            cmd(op::LINE, &xyz(-4, 5, -6)),
            cmd(op::POINT, &[]),
            cmd(op::CIRCLE, &7u16.to_le_bytes()),
            cmd(op::PUSH, &[]),
            cmd(op::TRANSLATE, &xyz(8, 9, 10)),
            cmd(op::ROTATE, &[1, 0x00, 0x40]),
            cmd(op::SCALE, &512u16.to_le_bytes()),
            cmd(op::POP, &[]),
            cmd(op::END, &[]),
            cmd(op::POINT, &[]),
        ]
        .concat();
        assert_eq!(
            all(&list),
            [
                Command::Colour([1, 2, 3, 4]),
                Command::Width(300),
                Command::Move([1, -2, 3]),
                Command::Line([-4, 5, -6]),
                Command::Point,
                Command::Circle(7),
                Command::Push,
                Command::Translate([8, 9, 10]),
                Command::Rotate(1, 0x4000),
                Command::Scale(512),
                Command::Pop,
            ]
        );
        let text: Vec<String> = all(&list).iter().map(|c| c.to_string()).collect();
        assert_eq!(
            text,
            ["colour 01020304", "width 300", "move 1 -2 3", "line -4 5 -6", "point", "circle 7", "push", "translate 8 9 10", "rotate y 16384", "scale 512", "pop"]
        );
        assert_eq!(opcodes(&list), [1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 8]);
    }

    #[test]
    fn termination_rules() {
        let point = cmd(op::POINT, &[]);
        let one = [Command::Point];
        assert_eq!(all(&point), one);
        assert_eq!(all(&[point.clone(), vec![op::END], point.clone()].concat()), one);
        for unknown in [0x0E, 0x80, 0xFF] {
            assert_eq!(all(&[point.clone(), vec![unknown], point.clone()].concat()), one);
        }
        // Operands cut off.
        for c in [cmd(op::LINE, &xyz(1, 2, 3)), cmd(op::COLOUR, &[1, 2, 3, 4]), cmd(op::WIDTH, &[1, 0]), cmd(op::ROTATE, &[0, 1, 0]), cmd(op::SCALE, &[0, 1]), cmd(op::CIRCLE, &[5, 0])] {
            for cut in 1..c.len() {
                assert_eq!(all(&[point.clone(), c[..cut].to_vec()].concat()), one, "{c:?} cut at {cut}");
            }
        }
        // A rotation about an axis above 2.
        assert_eq!(all(&[point.clone(), cmd(op::ROTATE, &[3, 0, 0]), point.clone()].concat()), one);
        // The stack: 8 pushes, the ninth ends the list; a pop with nothing saved.
        let pushes: Vec<u8> = std::iter::repeat_n(op::PUSH, 9).chain([op::POINT]).collect();
        assert_eq!(all(&pushes), vec![Command::Push; 8]);
        let mut balanced: Vec<u8> = std::iter::repeat_n(op::PUSH, 8).collect();
        balanced.extend(std::iter::repeat_n(op::POP, 8));
        balanced.extend([op::PUSH, op::POP, op::POP, op::POINT]);
        assert_eq!(all(&balanced).len(), 18);
        assert_eq!(all(&[op::POP, op::POINT]), []);
        assert_eq!(all(&[]), []);
    }

    /// Transforms compose as a matrix stack: the last given applies first.
    #[test]
    fn transform_composition() {
        let mut s = State::default();
        // Translate (100, 0, 0), then rotate a quarter turn about z, then
        // scale 2: the local point (10, 0, 0) is scaled to (20, 0, 0),
        // turned to (0, 20, 0) (x towards y), then moved to (100, 20, 0).
        for c in [Command::Translate([1600, 0, 0]), Command::Rotate(2, 0x4000), Command::Scale(512), Command::Move([160, 0, 0])] {
            s.step(&c);
        }
        assert!(close(s.point, [100.0, 20.0, 0.0]), "{:?}", s.point);
        // About x a quarter turn takes y to z; about y, z to x.
        let mut t = Transform::IDENTITY;
        t.rotate(0, 0.25);
        assert!(close(t.apply([0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]));
        let mut t = Transform::IDENTITY;
        t.rotate(1, 0.25);
        assert!(close(t.apply([0.0, 0.0, 1.0]), [1.0, 0.0, 0.0]));
        assert!(close(t.apply([1.0, 0.0, 0.0]), [0.0, 0.0, -1.0]));
        // An eighth of a turn about y, then a translation in the turned frame.
        let mut t = Transform::IDENTITY;
        t.rotate(1, 0.125);
        t.translate([0.0, 0.0, 10.0]);
        let h = 10.0 / 2f64.sqrt();
        assert!(close(t.apply([0.0; 3]), [h, 0.0, h]));
        // Push and pop restore the transform but not the current point,
        // which stays where it was put.
        let mut s = State::default();
        for c in [Command::Push, Command::Translate([160, 0, 0]), Command::Move([0, 160, 0]), Command::Pop] {
            s.step(&c);
        }
        assert!(close(s.point, [10.0, 10.0, 0.0]));
        assert_eq!(s.step(&Command::Line([0, 0, 0])), Some(Figure::Line([10.0, 10.0, 0.0], [0.0; 3])));
        // A circle lies in the local xy plane, about the current point.
        let mut s = State::default();
        for c in [Command::Move([1600, 1600, 0]), Command::Rotate(0, 0x4000)] {
            s.step(&c);
        }
        let Some(Figure::Circle(ring)) = s.step(&Command::Circle(160)) else { panic!() };
        assert_eq!(ring.len(), CIRCLE_CHORDS);
        assert!(close(ring[0], [110.0, 100.0, 0.0]));
        assert!(close(ring[16], [100.0, 100.0, 10.0]), "{:?}", ring[16]);
    }

    #[test]
    fn projection() {
        // On the glass: where the 2D layers would put it, for any eye.
        for eye in [(0.0, 0.0), (-160.0, 0.0), (160.0, 160.0), (37.5, -99.0)] {
            assert_eq!(to_glass([123.0, 45.0, 0.0], (EYE_X + eye.0, EYE_Y + eye.1)), (123.0, 45.0));
            let list = [cmd(op::MOVE, &xyz(1600, 800, 0)), cmd(op::LINE, &xyz(3200, 4800, 0))].concat();
            let got = beams(&list, (eye.0 as f32, eye.1 as f32));
            assert_eq!(got, [Primitive::Beam { x0: 1600, y0: 800, x1: 3200, y1: 4800, width: 16, rgba: [255; 4], fade: [255, 255], butt: 0 }]);
        }
        // 960 units behind the glass: half way to the eye's point.
        assert_eq!(to_glass([0.0, 0.0, 960.0], (EYE_X, EYE_Y)), (160.0, 120.0));
        assert_eq!(depth_cue(960.0), 0.5);
        assert_eq!(depth_cue(-400.0), 1.0);
        // Parallax: a far point moves with the eye, a near one against it.
        let far = to_glass([320.0, 240.0, 960.0], (EYE_X + 160.0, EYE_Y));
        let near = to_glass([320.0, 240.0, -480.0], (EYE_X + 160.0, EYE_Y));
        assert!(far.0 > 320.0 && near.0 < 320.0, "{far:?} {near:?}");
        // The depth cue at each end, interpolated when clipped to the screen.
        let list = [cmd(op::MOVE, &xyz(5120, 3840, 0)), cmd(op::LINE, &xyz(5120, 3840, 960 * 16))].concat();
        let got = beams(&list, (0.0, 0.0));
        assert_eq!(got, [Primitive::Beam { x0: 5120, y0: 3840, x1: 5120, y1: 3840, width: 16, rgba: [255; 4], fade: [255, 128], butt: 0 }]);
    }

    #[test]
    fn near_plane_clipping() {
        // Wholly behind the plane: nothing; across it: cut at z = −800.
        assert_eq!(near_clip([0.0, 0.0, -900.0], [0.0, 0.0, -800.0]), None);
        let (a, b) = near_clip([0.0, 0.0, -1000.0], [0.0, 10.0, 0.0]).unwrap();
        assert!(close(a, [0.0, 2.0, -800.0]) && close(b, [0.0, 10.0, 0.0]));
        // A line from the centre into the viewer: what is left ends where
        // the plane cuts it (6 times nearer, so 6 times further out).
        let list = [cmd(op::MOVE, &xyz(5120, 3840, 0)), cmd(op::LINE, &xyz(5440, 3840, -1000 * 16))].concat();
        let got = beams(&list, (0.0, 0.0));
        let Primitive::Beam { x0, x1, fade, .. } = got[0] else { panic!() };
        // The cut point: x = 320 + 20 × 0.8 = 336, seen at 320 + 16 × 6 = 416.
        assert_eq!((x0, x1, fade), (5120, 416 * 16, [255, 255]));
        assert!(beams(&[cmd(op::MOVE, &xyz(5120, 3840, -800 * 16)), cmd(op::POINT, &[])].concat(), (0.0, 0.0)).is_empty());
        assert_eq!(beams(&[cmd(op::MOVE, &xyz(5120, 3840, -799 * 16)), cmd(op::POINT, &[])].concat(), (0.0, 0.0)).len(), 1);
    }

    #[test]
    fn the_caps() {
        // A horizontal line across the screen, 1 unit wide, counts 640
        // units of length and 640 × 4 of ink (its box on the screen, 4
        // units high, is less than its stroke), whatever part of it lies
        // outside; 60 fit in the ink cap of 153,600, the 61st does not,
        // and nothing after it is drawn.
        let across = [cmd(op::MOVE, &xyz(-16000, 1600, 0)), cmd(op::LINE, &xyz(26000, 1600, 0))].concat();
        let mut list: Vec<u8> = std::iter::repeat_n(across.clone(), 61).flatten().collect();
        list.extend(cmd(op::POINT, &[]));
        let mut out = Vec::new();
        let p = project(&list, (0.0, 0.0), &mut out);
        assert_eq!((p.segments, p.length, p.ink, p.capped), (60, 60.0 * 640.0, INK_CAP, true));
        // Counted with the reference eye: the same when the eye moves.
        let p = project(&list, (160.0, 0.0), &mut Vec::new());
        assert_eq!((p.segments, p.capped), (60, true));
        // A hair line (1/16 unit) counts its box, 640 × (1/16 + 3): more
        // ink than length, so for any width the ink cap comes before the
        // length cap.
        let hair = [cmd(op::WIDTH, &[1, 0]), across.clone()].concat();
        let list = hair;
        assert!((project(&list, (0.0, 0.0), &mut Vec::new()).ink - 640.0 * (1.0 / 16.0 + 3.0)).abs() < 1e-6);
        // Off the screen, and behind the near plane, counts nothing.
        let off = [cmd(op::MOVE, &xyz(-1600, -1600, 0)), cmd(op::LINE, &xyz(-1600, 9000, 0))].concat();
        let list: Vec<u8> = std::iter::repeat_n(off, 1000).flatten().collect();
        assert_eq!(project(&list, (0.0, 0.0), &mut Vec::new()).length, 0.0);
        // A circle counts its 64 chords as they are seen, and 64 segments:
        // a circle of radius 100 at the glass about 627.9 units.
        let circle = [cmd(op::MOVE, &xyz(5120, 3840, 0)), cmd(op::CIRCLE, &1600u16.to_le_bytes())].concat();
        let mut out = Vec::new();
        let p = project(&circle, (0.0, 0.0), &mut out);
        let chord = 2.0 * 100.0 * (std::f64::consts::PI / 64.0).sin();
        assert!((p.length - 64.0 * chord).abs() < 1e-6, "{}", p.length);
        assert_eq!(p.segments, 64);
        assert!(out.iter().all(|b| matches!(b, Primitive::Beam { butt: 3, .. })));
        // As many such circles fit as their ink allows.
        let n = (INK_CAP / p.ink) as usize;
        let many: Vec<u8> = std::iter::repeat_n(circle.clone(), n + 2).flatten().collect();
        assert_eq!(project(&many, (0.0, 0.0), &mut Vec::new()).segments, n * 64);
        // By count: 256 circles of radius 0 are 16,384 segments; a point
        // more is not drawn. (Thin, and behind the screen, so that their
        // ink stays under its cap.)
        let dot = [cmd(op::WIDTH, &[1, 0]), cmd(op::MOVE, &xyz(5120, 3840, 0)), cmd(op::CIRCLE, &[0, 0])].concat();
        let mut list: Vec<u8> = std::iter::repeat_n(dot, 256).flatten().collect();
        list.extend(cmd(op::POINT, &[]));
        let p = project(&list, (0.0, 0.0), &mut Vec::new());
        assert_eq!((p.segments, p.capped), (16_384, true));
    }

    #[test]
    fn widths_and_colours() {
        let list = [cmd(op::WIDTH, &[0, 0]), cmd(op::POINT, &[]), cmd(op::WIDTH, &[0, 4]), cmd(op::COLOUR, &[9, 8, 7, 6]), cmd(op::POINT, &[])].concat();
        let got = beams(&list, (0.0, 0.0));
        let w: Vec<(u8, Rgba)> = got.iter().map(|b| if let Primitive::Beam { width, rgba, .. } = *b { (width, rgba) } else { panic!() }).collect();
        assert_eq!(w, [(1, [255; 4]), (255, [9, 8, 7, 6])]);
    }

    #[test]
    fn device_state() {
        let mut v = Vector3::default();
        assert_eq!(v.submit(&[]), 1);
        assert_eq!(v.submit(&vec![0; MAX_LIST + 1]), 1);
        assert_eq!(v.submit(&[op::POINT]), 0);
        let g = v.generation();
        assert_eq!(v.submit(&[op::POINT]), 0);
        assert_eq!(v.generation(), g);
        let mut s = Vec::new();
        v.write_state(&mut s);
        assert_eq!(s, [1, 0, 0, 0, op::POINT]);
        v.clear();
        assert!(v.list().is_empty() && v.generation() != g);
        assert!(v.dump().contains("-- 3d: 0 segments, length 0.0 units --"));
    }

    /// A Face command: its corners in screen units.
    fn face(corners: &[[f64; 3]]) -> Vec<u8> {
        let mut v = vec![op::FACE, corners.len() as u8];
        for c in corners {
            v.extend(xyz((c[0] * 16.0) as i16, (c[1] * 16.0) as i16, (c[2] * 16.0) as i16));
        }
        v
    }

    /// A line in screen units.
    fn line(a: [f64; 3], b: [f64; 3]) -> Vec<u8> {
        let s = |p: [f64; 3]| xyz((p[0] * 16.0) as i16, (p[1] * 16.0) as i16, (p[2] * 16.0) as i16);
        [cmd(op::MOVE, &s(a)), cmd(op::LINE, &s(b))].concat()
    }

    fn beam(x0: i16, y0: i16, x1: i16, y1: i16, fade: [u8; 2], butt: u8) -> Primitive {
        Primitive::Beam { x0, y0, x1, y1, width: 16, rgba: [255; 4], fade, butt }
    }

    /// A triangle on the glass: (300, 200), (400, 200), (300, 300). At
    /// y = 240 it covers x from 300 to 360.
    const TRIANGLE: [[f64; 3]; 3] = [[300.0, 200.0, 0.0], [400.0, 200.0, 0.0], [300.0, 300.0, 0.0]];

    #[test]
    fn face_command() {
        let f = face(&[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]]);
        assert_eq!(f.len(), 2 + 18);
        assert_eq!(all(&f), [Command::Face(3, [[16, 32, 48], [64, 80, 96], [112, 128, 144], [0; 3]])]);
        assert_eq!(all(&f)[0].to_string(), "face 3 16 32 48 64 80 96 112 128 144");
        let q = face(&[[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]]);
        assert_eq!(all(&q).len(), 1);
        // Any other count, or corners cut off, ends the list.
        let point = cmd(op::POINT, &[]);
        for n in [0u8, 1, 2, 5, 255] {
            assert_eq!(all(&[point.clone(), vec![op::FACE, n], vec![0; 30], point.clone()].concat()), [Command::Point], "n = {n}");
        }
        for cut in 1..q.len() {
            assert_eq!(all(&[point.clone(), q[..cut].to_vec()].concat()), [Command::Point]);
        }
        // A face draws nothing and leaves the current point; it is transformed.
        let mut s = State::default();
        s.step(&Command::Move([16, 16, 16]));
        s.step(&Command::Translate([160, 0, 0]));
        let Some(Figure::Face(c)) = s.step(&all(&f)[0]) else { panic!() };
        assert!(close(c[0], [11.0, 2.0, 3.0]) && close(s.point, [1.0, 1.0, 1.0]));
        assert_eq!(opcodes(&[f, point].concat()), [op::FACE, op::POINT]);
    }

    #[test]
    fn a_line_behind_a_triangle() {
        let tri = face(&TRIANGLE);
        // At z = 960, half way: x' = 320 + (x − 320) / 2, y' = 240. The line
        // from x = 0 to 640 is seen from 160 to 480; 300 to 360 is hidden.
        let behind = line([0.0, 240.0, 960.0], [640.0, 240.0, 960.0]);
        let want = [beam(2560, 3840, 4800, 3840, [128, 128], BUTT_END), beam(5760, 3840, 7680, 3840, [128, 128], BUTT_START)];
        assert_eq!(beams(&[tri.clone(), behind.clone()].concat(), (0.0, 0.0)), want);
        // Faces hide wherever they are in the list.
        assert_eq!(beams(&[behind.clone(), tri.clone()].concat(), (0.0, 0.0)), want);
        // From the eye 160 to the right the same line is seen from 240 to
        // 560: the triangle, on the glass, hides another part of it.
        assert_eq!(
            beams(&[tri.clone(), behind.clone()].concat(), (160.0, 0.0)),
            [beam(3840, 3840, 4800, 3840, [128, 128], BUTT_END), beam(5760, 3840, 8960, 3840, [128, 128], BUTT_START)]
        );
        // Wholly hidden: seen from 305 to 355.
        assert!(beams(&[tri.clone(), line([290.0, 240.0, 960.0], [390.0, 240.0, 960.0])].concat(), (0.0, 0.0)).is_empty());
        // In front of the triangle, or in its plane (z = 0), or along its
        // edge: never hidden, from any eye.
        for eye in [(0.0, 0.0), (-160.0, 0.0), (160.0, 160.0)] {
            for l in [line([0.0, 240.0, -100.0], [640.0, 240.0, -100.0]), line([0.0, 240.0, 0.0], [640.0, 240.0, 0.0]), line([300.0, 200.0, 0.0], [400.0, 200.0, 0.0])] {
                assert_eq!(beams(&[tri.clone(), l.clone()].concat(), eye), beams(&l, eye));
            }
        }
        // A line on the glass passing in front of a face behind it.
        let deep = face(&TRIANGLE.map(|c| [c[0], c[1], 100.0]));
        let glass = line([0.0, 240.0, 0.0], [640.0, 240.0, 0.0]);
        for eye in [(0.0, 0.0), (-160.0, 0.0), (160.0, 160.0)] {
            assert_eq!(beams(&[deep.clone(), glass.clone()].concat(), eye), [beam(0, 3840, 10240, 3840, [255, 255], 0)]);
        }
    }

    /// A line through the triangle's plane: the part beyond it by more
    /// than the tolerance is hidden.
    #[test]
    fn a_line_through_the_plane() {
        // From (340, 240, −480), seen at x' = 360, to (340, 240, 960), seen
        // at 330: all of it inside the triangle on the glass.
        let list = [face(&TRIANGLE), line([340.0, 240.0, -480.0], [340.0, 240.0, 960.0])].concat();
        let got = beams(&list, (0.0, 0.0));
        // Hidden from z = 0.125 on: seen at 320 + 20 × 960 / 960.125.
        let cut = 320.0 + 20.0 * 960.0 / 960.125;
        assert_eq!(cut, 339.99739617237336);
        // The cue along the glass from 1 (clamped) to 0.5, at the cut.
        let s = (360.0 - cut) / 30.0;
        let fade = ((1.0 - 0.5 * s) * 255.0f64).round() as u8;
        assert_eq!(fade, 170);
        assert_eq!(got, [beam(5760, 3840, 5440, 3840, [255, fade], BUTT_END)]);
        // The other way round: the cut is the piece's start.
        let list = [face(&TRIANGLE), line([340.0, 240.0, 960.0], [340.0, 240.0, -480.0])].concat();
        assert_eq!(beams(&list, (0.0, 0.0)), [beam(5440, 3840, 5760, 3840, [fade, 255], BUTT_START)]);
    }

    /// A 4-corner face is two triangles, 0-1-2 and 0-2-3: a bent quad
    /// hides as its two halves; a flat one has no seam.
    #[test]
    fn quads() {
        let quad = face(&[[300.0, 200.0, 0.0], [360.0, 200.0, 0.0], [360.0, 280.0, 0.0], [300.0, 280.0, 0.0]]);
        let halves = [face(&[[300.0, 200.0, 0.0], [360.0, 200.0, 0.0], [360.0, 280.0, 0.0]]), face(&[[300.0, 200.0, 0.0], [360.0, 280.0, 0.0], [300.0, 280.0, 0.0]])].concat();
        // Lines behind it, across the diagonal at many places.
        let mut lines = Vec::new();
        for i in 0..40 {
            let y = 190.0 + 2.5 * i as f64;
            lines.extend(line([0.0, 2.0 * y - 240.0, 960.0], [640.0, 2.0 * y - 240.0, 960.0]));
        }
        let got = beams(&[quad.clone(), lines.clone()].concat(), (0.0, 0.0));
        assert_eq!(got, beams(&[halves, lines.clone()].concat(), (0.0, 0.0)));
        // Each line crossing the quad (y' from 200 to 280) is cut once, at
        // x' = 300 and 360, with no sliver at the diagonal.
        let cut: Vec<_> = got.iter().filter(|b| matches!(b, Primitive::Beam { butt, .. } if *butt != 0)).collect();
        assert_eq!(cut.len(), 2 * 33, "{got:?}");
        assert!(cut.iter().all(|b| matches!(b, Primitive::Beam { x0, x1, .. } if *x1 == 4800 || *x0 == 5760)));
        // Bent: corner 3 pushed back hides lines between the halves' planes
        // only where the second half is in front of them.
        let bent = face(&[[300.0, 200.0, 0.0], [360.0, 200.0, 0.0], [360.0, 280.0, 0.0], [300.0, 280.0, 400.0]]);
        let l = line([200.0, 270.0, 100.0], [440.0, 210.0, 100.0]);
        let first = face(&[[300.0, 200.0, 0.0], [360.0, 200.0, 0.0], [360.0, 280.0, 0.0]]);
        let second = face(&[[300.0, 200.0, 0.0], [360.0, 280.0, 0.0], [300.0, 280.0, 400.0]]);
        assert_eq!(beams(&[bent, l.clone()].concat(), (0.0, 0.0)), beams(&[first, second, l].concat(), (0.0, 0.0)));
    }

    /// The faces of a solid cube and its 12 edges: from every eye, each
    /// edge of a face turned towards the eye is drawn whole, as without
    /// faces, and the edges between two faces turned away are not drawn.
    #[test]
    fn a_solid_cube() {
        let c = [320.0, 240.0, 300.0];
        let h = 100.0;
        let corner = |i: usize| [c[0] + if i & 1 != 0 { h } else { -h }, c[1] + if i & 2 != 0 { h } else { -h }, c[2] + if i & 4 != 0 { h } else { -h }];
        // Faces: the corner indices, the outward normal.
        let faces: [([usize; 4], [f64; 3]); 6] = [
            ([0, 1, 3, 2], [0.0, 0.0, -1.0]),
            ([4, 5, 7, 6], [0.0, 0.0, 1.0]),
            ([0, 1, 5, 4], [0.0, -1.0, 0.0]),
            ([2, 3, 7, 6], [0.0, 1.0, 0.0]),
            ([0, 2, 6, 4], [-1.0, 0.0, 0.0]),
            ([1, 3, 7, 5], [1.0, 0.0, 0.0]),
        ];
        let edges: Vec<(usize, usize)> = (0..8).flat_map(|a| [1, 2, 4].map(|bit| (a, a | bit))).filter(|&(a, b)| a != b).collect();
        assert_eq!(edges.len(), 12);
        let face_list: Vec<u8> = faces.iter().flat_map(|(f, _)| face(&f.map(corner))).collect();
        for eye in [(0.0f32, 0.0f32), (-160.0, 0.0), (160.0, 160.0), (-100.0, 50.0), (0.0, -160.0)] {
            let e = [EYE_X + eye.0 as f64, EYE_Y + eye.1 as f64, -EYE_DISTANCE];
            let toward = |n: [f64; 3], f: &[usize; 4]| dot(n, sub(e, corner(f[0]))) > 0.0;
            let mut want = Vec::new();
            let mut list = face_list.clone();
            for &(a, b) in &edges {
                let shown = faces.iter().any(|(f, n)| f.contains(&a) && f.contains(&b) && toward(*n, f));
                let l = line(corner(a), corner(b));
                if shown {
                    want.extend(beams(&l, eye));
                }
                list.extend(l);
            }
            assert!(want.len() >= 4);
            assert_eq!(beams(&list, eye), want, "eye {eye:?}");
            // The faces anywhere in the list, in any order.
            let edges_only = list[face_list.len()..].to_vec();
            let reversed: Vec<u8> = faces.iter().rev().flat_map(|(f, _)| face(&f.map(corner))).collect();
            assert_eq!(beams(&[edges_only.clone(), reversed].concat(), eye), want);
            let (a, b) = face_list.split_at(face_list.len() / 2);
            assert_eq!(beams(&[b, &edges_only[..], a].concat(), eye), want);
        }
    }

    /// A turning solid cube, from many eyes: exactly the edges of the faces
    /// turned towards the eye are drawn, each whole, with no stray pieces
    /// of the others.
    #[test]
    fn a_turning_cube() {
        let mut seed = 12345u32;
        let mut rnd = |n: u32| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) % n
        };
        let corner = |i: usize| [if i & 1 != 0 { 1600i16 } else { -1600 }, if i & 2 != 0 { 1600 } else { -1600 }, if i & 4 != 0 { 1600 } else { -1600 }];
        let quads: [[usize; 4]; 6] = [[0, 1, 3, 2], [4, 5, 7, 6], [0, 1, 5, 4], [2, 3, 7, 6], [0, 2, 6, 4], [1, 3, 7, 5]];
        let edges: Vec<(usize, usize)> = (0..8).flat_map(|a| [1, 2, 4].map(|bit| (a, a | bit))).filter(|&(a, b)| a != b).collect();
        for _ in 0..300 {
            let mut prefix = [cmd(op::TRANSLATE, &xyz(320 * 16, 240 * 16, (rnd(600) as i16 - 100) * 16))].concat();
            for axis in 0..3 {
                prefix.extend(cmd(op::ROTATE, &[axis, rnd(256) as u8, rnd(256) as u8]));
            }
            let eye = (rnd(321) as f32 - 160.0, rnd(321) as f32 - 160.0);
            let e = [EYE_X + eye.0 as f64, EYE_Y + eye.1 as f64, -EYE_DISTANCE];
            let mut state = State::default();
            for c in decode(&prefix) {
                state.step(&c);
            }
            let world = |i: usize| state.transform.apply(units(corner(i)));
            let centre = state.transform.apply([0.0; 3]);
            let toward = |q: &[usize; 4]| {
                let n = cross(sub(world(q[1]), world(q[0])), sub(world(q[2]), world(q[0])));
                let out = if dot(n, sub(world(q[0]), centre)) > 0.0 { n } else { n.map(|v| -v) };
                dot(out, sub(e, world(q[0]))) > 0.0
            };
            let mut list = prefix.clone();
            for q in &quads {
                list.push(op::FACE);
                list.push(4);
                q.iter().for_each(|&i| list.extend(corner(i).iter().flat_map(|v| v.to_le_bytes())));
            }
            let mut shown = prefix.clone();
            for &(a, b) in &edges {
                let l = [cmd(op::MOVE, &xyz(corner(a)[0], corner(a)[1], corner(a)[2])), cmd(op::LINE, &xyz(corner(b)[0], corner(b)[1], corner(b)[2]))].concat();
                if quads.iter().any(|q| q.contains(&a) && q.contains(&b) && toward(q)) {
                    shown.extend(&l);
                }
                list.extend(l);
            }
            assert_eq!(beams(&list, eye), beams(&shown, eye), "eye {eye:?}, list {prefix:?}");
        }
    }

    /// Random triangles and lines from random eyes, against a brute-force
    /// ray test of points along each line: away from where what is hidden
    /// changes, a point is drawn exactly when the rule says it is visible.
    #[test]
    fn against_brute_force() {
        let mut seed = 777u32;
        let mut rnd = |lo: f64, hi: f64| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            lo + (hi - lo) * (seed >> 8) as f64 / (1u32 << 24) as f64
        };
        let mut checked = 0;
        for _ in 0..20 {
            let mut list = Vec::new();
            let mut tris = Vec::new();
            for _ in 0..12 {
                let c = [rnd(80.0, 560.0), rnd(60.0, 420.0), rnd(-300.0, 600.0)];
                let corner = |r: &mut dyn FnMut(f64, f64) -> f64| [(c[0] + r(-120.0, 120.0)).round(), (c[1] + r(-120.0, 120.0)).round(), (c[2] + r(-200.0, 200.0)).round()];
                let t = [corner(&mut rnd), corner(&mut rnd), corner(&mut rnd)];
                list.extend(face(&t));
                tris.push(t);
            }
            let mut lines = Vec::new();
            for _ in 0..30 {
                let a = [rnd(0.0, 640.0).round(), rnd(0.0, 480.0).round(), rnd(-500.0, 900.0).round()];
                let b = [rnd(0.0, 640.0).round(), rnd(0.0, 480.0).round(), rnd(-500.0, 900.0).round()];
                list.extend(line(a, b));
                lines.push((a, b));
            }
            let eye = (rnd(-160.0, 160.0) as f32, rnd(-160.0, 160.0) as f32);
            let e = [EYE_X + eye.0 as f64, EYE_Y + eye.1 as f64, -EYE_DISTANCE];
            // The rule, point by point.
            let hidden = |p: [f64; 3]| {
                tris.iter().any(|t| {
                    let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
                    let l = dot(n, n).sqrt();
                    let (de, dp) = (dot(n, sub(e, t[0])) / l, dot(n, sub(p, t[0])) / l);
                    if de == 0.0 || dp * de.signum() > -HIDE_TOLERANCE {
                        return false;
                    }
                    // Where the ray from the eye meets the plane; inside?
                    let k = de / (de - dp);
                    let x = [e[0] + (p[0] - e[0]) * k, e[1] + (p[1] - e[1]) * k, e[2] + (p[2] - e[2]) * k];
                    let side = |i: usize| dot(cross(sub(t[(i + 1) % 3], t[i]), sub(x, t[i])), n);
                    x[2] > NEAR_Z && (0..3).all(|i| side(i) >= 0.0) || (0..3).all(|i| side(i) <= 0.0) && x[2] > NEAR_Z
                })
            };
            let mut out = Vec::new();
            let mut pieces = Vec::new();
            project_pieces(&list, eye, &mut out, &mut pieces);
            let mut at = 0;
            for (i, &(a, b)) in lines.iter().enumerate() {
                let n = u16::from_le_bytes([pieces[2 * i], pieces[2 * i + 1]]) as usize;
                let mine = &out[at..at + n];
                at += n;
                let Some((a, b)) = near_clip(a, b) else { continue };
                let (p, q) = (to_glass(a, (e[0], e[1])), to_glass(b, (e[0], e[1])));
                let glass = ((q.0 - p.0).powi(2) + (q.1 - p.1).powi(2)).sqrt();
                let (wa, wb) = (EYE_DISTANCE + a[2], EYE_DISTANCE + b[2]);
                let point = |s: f64| {
                    let u = s * wa / ((1.0 - s) * wb + s * wa);
                    [a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u, a[2] + (b[2] - a[2]) * u]
                };
                for k in 1..100 {
                    let s = k as f64 / 100.0;
                    let g = (p.0 + (q.0 - p.0) * s, p.1 + (q.1 - p.1) * s);
                    if g.0 < 0.0 || g.0 > 640.0 || g.1 < 0.0 || g.1 > 480.0 {
                        continue;
                    }
                    // Skip where the answer changes within 0.6 units.
                    let d = 0.6 / glass.max(1e-9);
                    let h = hidden(point(s));
                    if [s - d, s + d].iter().any(|&t| (0.0..=1.0).contains(&t) && hidden(point(t)) != h) {
                        continue;
                    }
                    let drawn = mine.iter().any(|b| {
                        let Primitive::Beam { x0, y0, x1, y1, .. } = *b else { return false };
                        let (x0, y0, x1, y1) = (x0 as f64 / 16.0, y0 as f64 / 16.0, x1 as f64 / 16.0, y1 as f64 / 16.0);
                        let (dx, dy) = (x1 - x0, y1 - y0);
                        let l2 = dx * dx + dy * dy;
                        let t = if l2 > 0.0 { (((g.0 - x0) * dx + (g.1 - y0) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
                        (g.0 - x0 - dx * t).hypot(g.1 - y0 - dy * t) < 0.1
                    });
                    assert_eq!(drawn, !h, "line {i} at {s}, eye {eye:?}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 20_000, "{checked}");
    }

    /// Only the part of a face in front of the near plane hides; a face
    /// off the screen hides nothing there is to draw.
    #[test]
    fn faces_at_the_near_plane_and_off_the_screen() {
        let l = line([0.0, 240.0, 0.0], [640.0, 240.0, 0.0]);
        // Wholly nearer than the plane: nothing is hidden.
        let near = face(&[[-500.0, -500.0, -850.0], [1100.0, -500.0, -850.0], [1100.0, 900.0, -850.0], [-500.0, 900.0, -850.0]]);
        assert_eq!(beams(&[near, l.clone()].concat(), (0.0, 0.0)), beams(&l, (0.0, 0.0)));
        // Across it: from z = −900 at x = 200 to −700 at x = 440, so cut at
        // x = 320 (z = −800), which is seen at 320: only what is to the
        // right of that is hidden.
        let across = face(&[[200.0, 0.0, -900.0], [440.0, 0.0, -700.0], [440.0, 480.0, -700.0], [200.0, 480.0, -900.0]]);
        assert_eq!(beams(&[across, l.clone()].concat(), (0.0, 0.0)), [beam(0, 3840, 5120, 3840, [255, 255], BUTT_END)]);
        // Off the screen.
        let off = face(&[[1000.0, 0.0, 0.0], [2000.0, 0.0, 0.0], [2000.0, 480.0, 0.0]]);
        let far = line([-640.0, 240.0, 960.0], [1280.0, 240.0, 960.0]);
        assert_eq!(beams(&[off, far.clone()].concat(), (0.0, 0.0)), beams(&far, (0.0, 0.0)));
    }

    /// Points are hidden whole; a circle chord by chord, its cut chords
    /// keeping butt ends.
    #[test]
    fn points_and_circles() {
        let point = |x: i16, z: i16| [cmd(op::MOVE, &xyz(x * 16, 240 * 16, z * 16)), cmd(op::POINT, &[])].concat();
        let tri = face(&TRIANGLE);
        assert!(beams(&[tri.clone(), point(330, 0), point(340, 960)].concat(), (0.0, 0.0)).len() == 1);
        // A circle of radius 100 at z = 480 (seen at radius 66.7) about the
        // centre, behind a face hiding x' < 330.
        let wall = face(&[[-100.0, -100.0, 0.0], [330.0, -100.0, 0.0], [330.0, 600.0, 0.0], [-100.0, 600.0, 0.0]]);
        let circle = [cmd(op::MOVE, &xyz(5120, 3840, 480 * 16)), cmd(op::CIRCLE, &1600u16.to_le_bytes())].concat();
        let got = beams(&[wall, circle].concat(), (0.0, 0.0));
        // Chords k = −14 to 13 whole, 14 and −15 cut at x' = 330.
        assert_eq!(got.len(), 30);
        assert!(got.iter().all(|b| matches!(b, Primitive::Beam { butt: 3, x0, x1, fade: [170, 170], .. } if *x0 >= 5280 && *x1 >= 5280)));
        let at_cut = got.iter().filter(|b| matches!(b, Primitive::Beam { x0, x1, .. } if *x0 == 5280 || *x1 == 5280)).count();
        assert_eq!(at_cut, 2);
    }

    #[test]
    fn face_caps() {
        let tiny = face(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        let point = [cmd(op::MOVE, &xyz(5120, 3840, 0)), cmd(op::POINT, &[])].concat();
        // 256 faces, then a point: drawn; a 257th face is not, nor what follows.
        let faces = |n: usize| -> Vec<u8> { std::iter::repeat_n(tiny.clone(), n).flatten().collect() };
        let p = project(&[faces(256), point.clone()].concat(), (0.0, 0.0), &mut Vec::new());
        assert_eq!((p.faces, p.segments, p.capped), (256, 1, false));
        let p = project(&[faces(257), point.clone()].concat(), (0.0, 0.0), &mut Vec::new());
        assert_eq!((p.faces, p.segments, p.capped), (256, 0, true));
        // Faces × segments at most 131,072: 64 faces and 2,048 points; one
        // point or face more is not drawn.
        let points = |n: usize| -> Vec<u8> { std::iter::repeat_n(point.clone(), n).flatten().collect() };
        let p = project(&[faces(64), points(2049)].concat(), (0.0, 0.0), &mut Vec::new());
        assert_eq!((p.faces, p.segments, p.capped), (64, 2048, true));
        let p = project(&[points(2048), faces(65), points(1)].concat(), (0.0, 0.0), &mut Vec::new());
        assert_eq!((p.faces, p.segments, p.capped), (64, 2048, true));
        // Without faces the segment cap alone.
        assert_eq!(project(&points(16_384), (0.0, 0.0), &mut Vec::new()).segments, 16_384);
        // The length is counted as if there were no faces: a line hidden
        // whole still counts.
        let l = line([0.0, 240.0, 960.0], [640.0, 240.0, 960.0]);
        let wall = face(&[[-100.0, -100.0, 0.0], [800.0, -100.0, 0.0], [800.0, 600.0, 0.0]]);
        let p = project(&[wall.clone(), l.clone(), wall].concat(), (0.0, 0.0), &mut Vec::new());
        assert_eq!((p.segments, p.length, p.faces), (1, 320.0, 2));
        // At most 16,384 segments after cutting: 256 strips in front of 512
        // lines would cut them into 131,000 pieces.
        let mut list = Vec::new();
        for i in 0..256 {
            let y = 1.0 + i as f64 * 1.875;
            list.extend(face(&[[-100.0, y, 0.0], [740.0, y, 0.0], [740.0, y + 0.9375, 0.0], [-100.0, y + 0.9375, 0.0]]));
        }
        for i in 0..512 {
            let x = i as f64 * 1.25;
            list.extend(line([x, 0.0, 100.0], [x, 480.0, 100.0]));
        }
        let mut out = Vec::new();
        let p = project(&list, (0.0, 0.0), &mut out);
        assert_eq!((p.segments, out.len(), p.faces, p.capped), (16_384, 16_384, 256, true));
    }

    /// The pieces of each figure, for a list with faces.
    #[test]
    fn pieces_per_figure() {
        let list = [face(&TRIANGLE), line([0.0, 240.0, 960.0], [640.0, 240.0, 960.0]), cmd(op::POINT, &[])].concat();
        let mut pieces = Vec::new();
        project_pieces(&list, (0.0, 0.0), &mut Vec::new(), &mut pieces);
        assert_eq!(pieces, [2, 0, 1, 0]);
        project_pieces(&list[20..], (0.0, 0.0), &mut Vec::new(), &mut pieces);
        assert!(pieces.is_empty());
    }

    /// A Screen command: handle and width in screen units.
    fn screen(h: u8, w: f64) -> Vec<u8> {
        cmd(op::SCREEN, &[&[h][..], &((w * 16.0) as u16).to_le_bytes()].concat())
    }

    fn translate(x: f64, y: f64, z: f64) -> Vec<u8> {
        cmd(op::TRANSLATE, &xyz((x * 16.0) as i16, (y * 16.0) as i16, (z * 16.0) as i16))
    }

    /// The panels of a list with these Screens in effect, from `eye`.
    fn panels(list: &[u8], screens: u8, eye: (f32, f32)) -> (Vec<Panel>, Vec<Primitive>, Projection) {
        let (mut out, mut panels) = (Vec::new(), Vec::new());
        let scene = SceneView { screens, view: Transform::IDENTITY };
        let p = project_scene(list, eye, &scene, &mut out, &mut Vec::new(), &mut panels);
        (panels, out, p)
    }

    fn near(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn screen_command() {
        let list = [screen(3, 100.0), screen(8, 0.0)].concat();
        assert_eq!(all(&list), [Command::Screen(3, 1600), Command::Screen(8, 0)]);
        assert_eq!(all(&list)[0].to_string(), "screen 3 1600");
        assert_eq!(opcodes(&list), [op::SCREEN, op::SCREEN]);
        // A handle outside 1-8 ends the list, as do cut-off operands.
        for bad in [0u8, 9, 255] {
            assert_eq!(all(&[screen(1, 10.0), cmd(op::SCREEN, &[bad, 0, 1]), cmd(op::POINT, &[])].concat()).len(), 1);
        }
        assert_eq!(all(&[op::SCREEN, 1, 0]), []);
        assert_eq!(screens_in(&[screen(3, 1.0), screen(3, 2.0), screen(1, 0.0)].concat()), 0b101);
        assert_eq!(screens_in(&[cmd(op::POP, &[]), screen(3, 1.0)].concat()), 0, "after the end");
        // The current point stays; the panel lies about the origin.
        let mut s = State::default();
        s.step(&Command::Move([16, 32, 48]));
        let Some(Figure::Screen(2, c)) = s.step(&Command::Screen(2, 640)) else { panic!() };
        assert_eq!(c, [[-20.0, -15.0, 0.0], [20.0, -15.0, 0.0], [20.0, 15.0, 0.0], [-20.0, 15.0, 0.0]]);
        assert_eq!(s.point, [1.0, 2.0, 3.0]);
    }

    /// Panel corners on the glass against values worked out by hand.
    #[test]
    fn panel_corners() {
        let glass = |p: &Panel, eye: (f64, f64)| p.corners.map(|c| to_glass(c, (EYE_X + eye.0, EYE_Y + eye.1)));
        // The whole screen on the glass: the same from every eye.
        let list = [translate(320.0, 240.0, 0.0), screen(1, 640.0)].concat();
        for eye in [(0.0, 0.0), (160.0, -160.0)] {
            let (p, ..) = panels(&list, 1, (eye.0 as f32, eye.1 as f32));
            let g = glass(&p[0], eye);
            assert!(near(g[0], (0.0, 0.0)) && near(g[1], (640.0, 0.0)) && near(g[2], (640.0, 480.0)) && near(g[3], (0.0, 480.0)), "{g:?}");
            assert!(p[0].front);
        }
        // 960 units behind the glass, 320 wide: half its size, towards
        // the eye's point.
        let list = [translate(320.0, 240.0, 960.0), screen(1, 320.0)].concat();
        let (p, ..) = panels(&list, 1, (0.0, 0.0));
        let g = glass(&p[0], (0.0, 0.0));
        assert!(near(g[0], (240.0, 180.0)) && near(g[2], (400.0, 300.0)), "{g:?}");
        let (p, ..) = panels(&list, 1, (160.0, 0.0));
        let g = glass(&p[0], (160.0, 0.0));
        assert!(near(g[0], (320.0, 180.0)) && near(g[1], (480.0, 180.0)), "{g:?}");
        // Scaled: 2 × 100 wide about (100, 100).
        let list = [translate(100.0, 100.0, 0.0), cmd(op::SCALE, &512u16.to_le_bytes()), screen(1, 100.0)].concat();
        let (p, ..) = panels(&list, 1, (0.0, 0.0));
        assert_eq!(p[0].corners, [[0.0, 25.0, 0.0], [200.0, 25.0, 0.0], [200.0, 175.0, 0.0], [0.0, 175.0, 0.0]]);
        // A quarter turn about y: its left edge 640 deep, its right 320;
        // seen from 160 units left of the reference eye, its front.
        let list = [translate(320.0, 240.0, 480.0), cmd(op::ROTATE, &[1, 0x00, 0x40]), screen(1, 320.0)].concat();
        let (p, ..) = panels(&list, 1, (-160.0, 0.0));
        assert!(close(p[0].corners[0], [320.0, 120.0, 640.0]) && close(p[0].corners[1], [320.0, 120.0, 320.0]), "{:?}", p[0].corners);
        let g = glass(&p[0], (-160.0, 0.0));
        assert!(near(g[0], (256.0, 168.0)) && near(g[1], (280.0, 150.0)), "{g:?}");
        assert!(p[0].front);
        let (p, ..) = panels(&list, 1, (160.0, 0.0));
        assert!(!p[0].front, "from the right: its back");
        // Edge on from the reference eye: no front, and no pointer hit.
        let (p, ..) = panels(&list, 1, (0.0, 0.0));
        assert!(!p[0].front);
        assert_eq!(panel_hit(&p, (0.0, 0.0), 320.0, 200.0), None);
    }

    /// Screens count as faces, hide what is behind them, are drawn far to
    /// near, and only the first of a handle and only those in effect count.
    #[test]
    fn screens_in_a_scene() {
        let behind = line([0.0, 240.0, 100.0], [640.0, 240.0, 100.0]);
        let before = line([0.0, 200.0, -100.0], [640.0, 200.0, -100.0]);
        let full = [cmd(op::PUSH, &[]), translate(320.0, 240.0, 0.0), screen(1, 640.0), cmd(op::POP, &[])].concat();
        let list = [full.clone(), behind.clone(), before.clone()].concat();
        // Not in effect (the list of a machine that is not the shell): no
        // face, both lines drawn whole.
        let (p, out, proj) = panels(&list, 0, (0.0, 0.0));
        assert_eq!((p.len(), out.len(), proj.faces), (0, 2, 0));
        assert_eq!(beams(&list, (0.0, 0.0)), out);
        // In effect: a face that hides the line behind it.
        let (p, out, proj) = panels(&list, 1, (0.0, 0.0));
        assert_eq!((p.len(), proj.faces), (1, 1));
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Primitive::Beam { y0: 3126, .. }), "the line in front: {out:?}");
        // A second Screen of the same handle is ignored; another handle's is
        // a panel of its own, drawn first if it is further away.
        let far = [translate(0.0, 0.0, 900.0), screen(2, 100.0)].concat();
        let list = [full.clone(), screen(1, 10.0), far, behind].concat();
        let (p, _, proj) = panels(&list, 0b11, (0.0, 0.0));
        assert_eq!(p.iter().map(|p| p.handle).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(proj.faces, 2);
        let (p, _, _) = panels(&list, 0b10, (0.0, 0.0));
        assert_eq!(p.iter().map(|p| p.handle).collect::<Vec<_>>(), [2]);
        // The face caps: 256 faces, then a Screen is not drawn.
        let mut many: Vec<u8> = std::iter::repeat_n(face(&TRIANGLE), 256).flatten().collect();
        many.extend(screen(1, 100.0));
        let (p, _, proj) = panels(&many, 1, (0.0, 0.0));
        assert_eq!((p.len(), proj.faces, proj.capped), (0, 256, true));
    }

    /// The pointer against the panels (SPEC §machines.input).
    #[test]
    fn pointer_hits() {
        let full = [translate(320.0, 240.0, 0.0), screen(1, 640.0)].concat();
        let (p, ..) = panels(&full, 1, (0.0, 0.0));
        assert_eq!(panel_hit(&p, (0.0, 0.0), 320.5, 240.5), Some((1, Some((320, 240)))));
        assert_eq!(panel_hit(&p, (0.0, 0.0), 0.5, 0.5), Some((1, Some((0, 0)))));
        assert_eq!(panel_hit(&p, (90.0, -40.0), 639.5, 479.5), Some((1, Some((639, 479)))), "on the glass: any eye");
        assert_eq!(panel_hit(&p, (0.0, 0.0), 640.5, 240.0), None);
        // A small one in front of a large one far away: the nearer wins.
        let list = [translate(320.0, 240.0, 0.0), screen(1, 320.0), translate(0.0, 0.0, 960.0), screen(2, 1280.0)].concat();
        let (p, ..) = panels(&list, 0b11, (0.0, 0.0));
        assert_eq!(panel_hit(&p, (0.0, 0.0), 320.5, 240.5), Some((1, Some((321, 241)))));
        assert_eq!(panel_hit(&p, (0.0, 0.0), 10.5, 10.5), Some((2, Some((10, 10)))));
        // The far one seen from an eye moved right: a point on the glass
        // meets it further right.
        let (p, ..) = panels(&list, 0b10, (160.0, 0.0));
        assert_eq!(panel_hit(&p, (160.0, 0.0), 10.5, 10.5), None);
        assert_eq!(panel_hit(&p, (160.0, 0.0), 200.5, 10.5), Some((2, Some((120, 10)))));
        // From behind (half a turn about y): hit, but not delivered.
        let list = [translate(320.0, 240.0, 100.0), cmd(op::ROTATE, &[1, 0x00, 0x80]), screen(3, 320.0)].concat();
        let (p, ..) = panels(&list, 0b100, (0.0, 0.0));
        assert!(!p[0].front);
        assert_eq!(panel_hit(&p, (0.0, 0.0), 320.0, 240.0), Some((3, None)));
        // Behind the near plane: not hit.
        let list = [translate(320.0, 240.0, -900.0), screen(1, 640.0)].concat();
        let (p, ..) = panels(&list, 1, (0.0, 0.0));
        assert_eq!(panel_hit(&p, (0.0, 0.0), 320.5, 240.5), None);
    }

    /// The view moves a panel, turned and far away, onto the glass over the
    /// target, and is the scene as listed at its start.
    #[test]
    fn view_onto_the_glass() {
        let list = [translate(320.0, 240.0, 480.0), cmd(op::ROTATE, &[1, 0x00, 0x30]), screen(1, 320.0)].concat();
        let c = screen_corners(&list, 1).unwrap();
        assert_eq!(screen_corners(&list, 2), None);
        let target = (17.0, 23.0, 605.0, 453.0);
        let v = view_transform(&c, target, 0.0);
        for k in 0..4 {
            assert!((0..3).all(|i| (v.apply(c[k])[i] - c[k][i]).abs() < 1e-9));
        }
        let v = view_transform(&c, target, 1.0);
        // Centred on the target: 453.75 high about y = 249.5.
        let want = [[17.0, 22.625], [622.0, 22.625], [622.0, 476.375], [17.0, 476.375]];
        for k in 0..4 {
            let p = v.apply(c[k]);
            assert!((p[0] - want[k][0]).abs() < 1e-6 && (p[1] - want[k][1]).abs() < 1e-6 && p[2].abs() < 1e-6, "{k}: {p:?}");
        }
        // Half way: the centre half way, the width the geometric mean.
        let v = view_transform(&c, target, 0.5);
        let m: Vec<[f64; 3]> = c.iter().map(|&p| v.apply(p)).collect();
        let mc = centre(&[m[0], m[1], m[2], m[3]]);
        let c0 = centre(&c);
        assert!((mc[0] - (c0[0] + 319.5) / 2.0).abs() < 1e-6 && (mc[2] - c0[2] / 2.0).abs() < 1e-6, "{mc:?}");
        let w = (0..3).map(|i| (m[1][i] - m[0][i]).powi(2)).sum::<f64>().sqrt();
        assert!((w - (320.0f64 * 605.0).sqrt()).abs() < 1e-6, "{w}");
        assert_eq!((ease(0), ease(2048), ease(4096), ease(5000)), (0.0, 0.5, 1.0, 1.0));
    }

    /// A Surface command: the source rectangle (screen units) and the
    /// panel's width in screen units.
    fn surface(r: [u16; 4], w: f64) -> Vec<u8> {
        let mut v = vec![op::SURFACE];
        r.iter().for_each(|x| v.extend(x.to_le_bytes()));
        v.extend(((w * 16.0) as u16).to_le_bytes());
        v
    }

    /// The Surfaces of a list as `surfaces` gives them, and its segments.
    fn surfaced(list: &[u8], eye: (f32, f32)) -> (Vec<Panel>, Vec<Primitive>, Projection) {
        let (mut out, mut panels) = (Vec::new(), Vec::new());
        let p = project_scene(list, eye, &SceneView::LISTED, &mut out, &mut Vec::new(), &mut panels);
        let (again, q) = surfaces(list, eye);
        assert_eq!(again, panels);
        assert_eq!((q.surfaces, q.rects, q.faces, q.capped), (p.surfaces, p.rects, p.faces, p.capped));
        (panels, out, p)
    }

    /// Decoding, the text form, where a cut-off Surface ends the list, the
    /// clamped source and the corners worked out by hand.
    #[test]
    fn surface_command() {
        let list = [surface([1, 2, 3, 4], 10.0), surface([700, 500, 5, 5], 0.0)].concat();
        assert_eq!(all(&list), [Command::Surface([1, 2, 3, 4], 160), Command::Surface([700, 500, 5, 5], 0)]);
        assert_eq!(all(&list)[0].to_string(), "surface 1 2 3 4 160");
        assert_eq!(opcodes(&list), [op::SURFACE, op::SURFACE]);
        assert_eq!(all(&surface([1, 2, 3, 4], 1.0)[..10]), [], "cut off");
        assert!(has_surface(&list) && !has_surface(&cmd(op::POINT, &[])));
        // Clamped as the page's rectangle is.
        assert_eq!(clamp_rect([600, 470, 100, 100]), [600, 470, 40, 10]);
        assert_eq!(clamp_rect([700, 500, 5, 5]), [640, 480, 0, 0]);
        assert_eq!(clamp_rect([0, 0, 640, 480]), WHOLE_SCREEN);
        // 160 wide about (320, 240), the source's shape: 60 high.
        let mut s = State::default();
        s.step(&Command::Move([16, 32, 48]));
        s.step(&Command::Translate([320 * 16, 240 * 16, 0]));
        let Some(Figure::Surface(src, 2560, c)) = s.step(&Command::Surface([0, 0, 320, 120], 2560)) else { panic!() };
        assert_eq!(src, [0, 0, 320, 120]);
        assert_eq!(c, [[240.0, 210.0, 0.0], [400.0, 210.0, 0.0], [400.0, 270.0, 0.0], [240.0, 270.0, 0.0]]);
        assert_eq!(s.point, [1.0, 2.0, 3.0], "the current point stays");
        // A quarter turn about y, 480 deep: the left edge 100 deeper, the
        // right 100 nearer; clamped source 40 × 10, so 50 high.
        let list = [translate(320.0, 240.0, 480.0), cmd(op::ROTATE, &[1, 0x00, 0x40]), surface([600, 470, 100, 100], 200.0)].concat();
        let (p, _, proj) = surfaced(&list, (0.0, 0.0));
        assert_eq!(proj.surface_rects(), [[600, 470, 40, 10]]);
        assert!(close(p[0].corners[0], [320.0, 215.0, 580.0]) && close(p[0].corners[1], [320.0, 215.0, 380.0]), "{:?}", p[0].corners);
        assert!(close(p[0].corners[2], [320.0, 265.0, 380.0]) && close(p[0].corners[3], [320.0, 265.0, 580.0]), "{:?}", p[0].corners);
        assert_eq!((p[0].handle, p[0].source), (0, [600, 470, 40, 10]));
        assert!(p[0].is_surface());
        // The dump names it and where its corners are.
        let mut v = Vector3::default();
        v.submit(&[translate(320.0, 240.0, 0.0), surface([0, 0, 320, 120], 160.0)].concat());
        assert!(v.dump().contains("surface 0 0 320 120 2560\n"), "{}", v.dump());
        assert!(v.dump().contains("-- surface 0 0 320 120 (240.0, 210.0) (400.0, 210.0) (400.0, 270.0) (240.0, 270.0) front --\n"), "{}", v.dump());
    }

    /// Only the first four Surfaces with a source and a width take effect;
    /// the others draw nothing, hide nothing and are not counted.
    #[test]
    fn surface_cap() {
        let behind = line([0.0, 240.0, 100.0], [640.0, 240.0, 100.0]);
        let small = |x: f64| [cmd(op::PUSH, &[]), translate(x, 240.0, 0.0), surface([0, 0, 64, 48], 40.0), cmd(op::POP, &[])].concat();
        let empty = [surface([10, 10, 0, 5], 40.0), surface([10, 10, 5, 0], 40.0), surface([10, 10, 5, 5], 0.0), surface([640, 0, 9, 9], 9.0)].concat();
        let list = [empty, small(50.0), small(150.0), small(250.0), small(350.0), small(450.0), behind.clone()].concat();
        let (p, out, proj) = surfaced(&list, (0.0, 0.0));
        assert_eq!((proj.surfaces, proj.faces, p.len()), (4, 4, 4));
        assert!(p.iter().all(|p| p.source == [0, 0, 64, 48]));
        // The line is cut where the first four are, not at the fifth (430
        // to 470).
        let covered = |x: f64| {
            out.iter().all(|b| {
                let Primitive::Beam { x0, x1, .. } = *b else { panic!() };
                let (a, b) = (x0.min(x1) as f64 / 16.0, x0.max(x1) as f64 / 16.0);
                !(a < x && x < b)
            })
        };
        assert!(covered(50.0) && covered(150.0) && covered(350.0), "{out:?}");
        assert!(!covered(450.0), "the fifth hides nothing: {out:?}");
        // The face caps: 256 faces, then a Surface is not drawn.
        let mut many: Vec<u8> = std::iter::repeat_n(face(&TRIANGLE), 256).flatten().collect();
        many.extend(small(320.0));
        let (p, _, proj) = surfaced(&many, (0.0, 0.0));
        assert_eq!((p.len(), proj.faces, proj.surfaces, proj.capped), (0, 256, 0, true));
        // 255 faces: the Surface is the 256th.
        let mut fits: Vec<u8> = std::iter::repeat_n(face(&TRIANGLE), 255).flatten().collect();
        fits.extend(small(320.0));
        let (_, _, proj) = surfaced(&fits, (0.0, 0.0));
        assert_eq!((proj.faces, proj.surfaces, proj.capped), (256, 1, false));
    }

    /// A Surface hides exactly as a Face of its four corners, from any eye,
    /// and lines in front of it are drawn; a list without Surfaces gives no
    /// panels and no rectangles.
    #[test]
    fn surfaces_hide_as_faces() {
        let lines = [
            line([0.0, 240.0, 100.0], [640.0, 240.0, 100.0]),
            line([0.0, 200.0, -100.0], [640.0, 260.0, -100.0]),
            line([300.0, 0.0, 300.0], [340.0, 480.0, -300.0]),
            cmd(op::MOVE, &xyz(320 * 16, 250 * 16, 50 * 16)),
            cmd(op::POINT, &[]),
        ]
        .concat();
        let place = [translate(320.0, 240.0, 40.0), cmd(op::ROTATE, &[1, 0x00, 0x10]), cmd(op::ROTATE, &[0, 0x00, 0x08])].concat();
        let with = [cmd(op::PUSH, &[]), place.clone(), surface([100, 100, 300, 150], 400.0), cmd(op::POP, &[]), lines.clone()].concat();
        let local = [[-200.0, -100.0, 0.0], [200.0, -100.0, 0.0], [200.0, 100.0, 0.0], [-200.0, 100.0, 0.0]];
        let as_face = [cmd(op::PUSH, &[]), place.clone(), face(&local), cmd(op::POP, &[]), lines.clone()].concat();
        for eye in [(0.0, 0.0), (-160.0, 0.0), (160.0, 120.0), (37.5, -100.0)] {
            let (_, out, proj) = surfaced(&with, eye);
            assert_eq!(out, beams(&as_face, eye), "eye {eye:?}");
            assert_eq!((proj.faces, proj.surfaces), (1, 1));
            assert!(out.len() > 3, "something is cut: {out:?}");
        }
        // Without: nothing of the Surfaces.
        let (p, out, proj) = surfaced(&lines, (0.0, 0.0));
        assert_eq!((p.len(), proj.surfaces, proj.surface_rects().len()), (0, 0, 0));
        assert_eq!(out, beams(&lines, (0.0, 0.0)));
    }

    /// The pointer over a Surface goes to the point of its source rectangle
    /// (SPEC §vector3d.list): the centre, the corners, from a moved eye, from behind,
    /// two overlapping, over none.
    #[test]
    fn surface_pointer_mapping() {
        // (120, 140) to (520, 340) on the glass, showing (100, 50) 200 × 100.
        let flat = [translate(320.0, 240.0, 0.0), surface([100, 50, 200, 100], 400.0)].concat();
        assert_eq!(surface_pointer(&flat, (0.0, 0.0), 320, 240), (200, 100));
        assert_eq!(surface_pointer(&flat, (0.0, 0.0), 120, 140), (100, 50));
        assert_eq!(surface_pointer(&flat, (0.0, 0.0), 519, 339), (299, 149));
        assert_eq!(surface_pointer(&flat, (160.0, -160.0), 519, 339), (299, 149), "on the glass: any eye");
        assert_eq!(surface_pointer(&flat, (0.0, 0.0), 520, 240), (520, 240), "over none");
        assert_eq!(surface_pointer(&flat, (0.0, 0.0), 119, 140), (119, 140));
        // 960 behind the glass, the whole screen 640 wide: half size about
        // the eye's point.
        let far = [translate(320.0, 240.0, 960.0), surface(WHOLE_SCREEN, 640.0)].concat();
        assert_eq!(surface_pointer(&far, (0.0, 0.0), 320, 240), (321, 241));
        assert_eq!(surface_pointer(&far, (160.0, 0.0), 320, 240), (161, 241));
        assert_eq!(surface_pointer(&far, (0.0, 0.0), 100, 240), (100, 240), "outside it");
        // Seen from behind (half a turn about y): the glass position.
        let back = [translate(320.0, 240.0, 100.0), cmd(op::ROTATE, &[1, 0x00, 0x80]), surface([0, 0, 64, 48], 320.0)].concat();
        let (p, ..) = surfaced(&back, (0.0, 0.0));
        assert!(!p[0].front);
        assert_eq!(panel_hit(&p, (0.0, 0.0), 320.5, 240.5), Some((0, None)));
        assert_eq!(surface_pointer(&back, (0.0, 0.0), 320, 240), (320, 240));
        // Two overlapping: the nearer wins, wherever it is in the list.
        let near = [cmd(op::PUSH, &[]), translate(320.0, 240.0, -100.0), surface([0, 0, 100, 100], 100.0), cmd(op::POP, &[])].concat();
        let list = [near.clone(), far.clone()].concat();
        let (p, ..) = surfaced(&list, (0.0, 0.0));
        assert_eq!(p.iter().map(|p| p.source[2]).collect::<Vec<_>>(), [640, 100], "far to near");
        // The near one: 100 wide at z = −100, so 960/860 as wide on the glass.
        let at = surface_pointer(&list, (0.0, 0.0), 320, 240);
        assert_eq!(at, (50, 50));
        assert_eq!(surface_pointer(&[cmd(op::PUSH, &[]), far, cmd(op::POP, &[]), near].concat(), (0.0, 0.0), 320, 240), at);
        assert_eq!(surface_pointer(&list, (0.0, 0.0), 200, 240), (81, 241), "only the far one there");
    }
}
