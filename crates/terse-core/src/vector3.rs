//! Device 9, 3D vector display (SPEC 13).
//!
//! The device only validates and copies the display list; the style and
//! persistence it sets are device 3's ([`crate::Vector`]), shared by both
//! displays. [`decode`] turns a stored list into its commands (SPEC 13.3
//! termination rules, the transform stack's limits), and [`project`] turns
//! a list and an eye into the line segments a host draws on the glass, with
//! the caps of SPEC 13.4, the depth cue of SPEC 13.5 and what faces hide
//! cut away (SPEC 13.3). Every renderer
//! draws from [`project`], so they all draw the same segments.

use std::fmt;

use crate::vector::{Primitive, Rgba};

/// The device's slot (SPEC 2).
pub const SLOT: u8 = 9;
/// Longest list a submit accepts (SPEC 13.2).
pub const MAX_LIST: usize = 16_384;
/// Transforms a list can save with Push (SPEC 13.3).
pub const STACK_DEPTH: usize = 8;
/// The reference eye: (320, 240, −960) in screen units (SPEC 13.1).
pub const EYE_X: f64 = 320.0;
pub const EYE_Y: f64 = 240.0;
pub const EYE_DISTANCE: f64 = 960.0;
/// Points with z at or nearer than this (screen units) are not drawn.
pub const NEAR_Z: f64 = -800.0;
/// How far a host may move the eye from the reference eye, on each axis.
pub const MAX_EYE_OFFSET: f32 = 160.0;
/// Total length (screen units, on the glass, reference eye, clipped to the
/// screen) one list may draw (SPEC 13.4). Provisional.
pub const LENGTH_CAP: f64 = 65_536.0;
/// Segments one list may draw: a line or point is one, a circle
/// [`CIRCLE_CHORDS`] (SPEC 13.4). Provisional.
pub const SEGMENT_CAP: usize = 16_384;
/// The chords a circle is drawn as.
pub const CIRCLE_CHORDS: usize = 64;
/// The widest line drawn, in sixteenths (that of the 2D display).
pub const MAX_WIDTH: u16 = 255;
/// How far outside the screen (screen units) segments are kept for
/// drawing, so that their ends and glow beyond the edge are drawn as they
/// would be without the cut.
const DRAW_MARGIN: f64 = 32.0;

/// Display-list opcodes (SPEC 13.3).
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
}

/// Bit of [`Primitive::Beam`]'s `butt`: the end continues into the next
/// chord of a circle, so it has no round cap and no vertex dwell.
pub const BUTT_START: u8 = 1;
pub const BUTT_END: u8 = 2;

/// The device state (SPEC 13.6): the copy of the list. The style and the
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
    pub(crate) fn write_state(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.list.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.list);
    }

    pub(crate) fn restore(&mut self, r: &mut crate::snapshot::Reader) -> Result<(), crate::snapshot::RestoreError> {
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
        out
    }
}

/// A point of the list, in sixteenths of a screen unit.
pub type Xyz = [i16; 3];

/// One command of a list (SPEC 13.3), operands as given.
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
        op::ROTATE => 3,
        _ => return None,
    })
}

/// Decodes a list (SPEC 13.3): its commands up to where it ends (an End,
/// the end of the bytes, an unknown opcode, cut-off operands, a Rotate
/// about an axis above 2, a Push with 8 transforms saved or a Pop with none).
pub fn decode(list: &[u8]) -> Decoder<'_> {
    Decoder { list, at: 0, depth: 0 }
}

/// Iterator over the commands of a list; see [`decode`].
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

    /// Translate (SPEC 13.3), in screen units.
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

/// The list's drawing state at each command (SPEC 13.3): colour, width,
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
        }
        None
    }
}

/// (sin, cos) of k/64 of a turn, k = 0 to 63: a circle's points.
fn unit_circle() -> &'static [(f64, f64); CIRCLE_CHORDS] {
    static TABLE: std::sync::OnceLock<[(f64, f64); CIRCLE_CHORDS]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| std::array::from_fn(|k| (k as f64 / CIRCLE_CHORDS as f64 * std::f64::consts::TAU).sin_cos()))
}

/// The depth cue at depth `z` (SPEC 13.5): 960 / (960 + z), at most 1.
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

/// The screen (SPEC 7), in screen units.
const SCREEN: (f64, f64, f64, f64) = (0.0, 0.0, 640.0, 480.0);

/// The length on the glass, seen from the reference eye, of the part of
/// a–b in front of the near plane and on the screen (SPEC 13.4).
pub fn counted_length(a: [f64; 3], b: [f64; 3]) -> f64 {
    let Some((a, b)) = near_clip(a, b) else { return 0.0 };
    let reference = (EYE_X, EYE_Y);
    let (p, q) = (to_glass(a, reference), to_glass(b, reference));
    match clip_rect(p, q, SCREEN) {
        Some((t0, t1)) => (t1 - t0) * ((q.0 - p.0).powi(2) + (q.1 - p.1).powi(2)).sqrt(),
        None => 0.0,
    }
}

/// Faces one list may give (SPEC 13.4).
pub const FACE_CAP: usize = 256;
/// The most faces × segments one list may give (SPEC 13.4): it bounds the
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
    /// Their counted length (SPEC 13.4), in screen units, as if there were
    /// no faces.
    pub length: f64,
    /// Faces in effect.
    pub faces: usize,
    /// A command was stopped by a cap (or a segment by the cap on what is
    /// drawn after cutting).
    pub capped: bool,
}

/// The segments a host draws for `list` (SPEC 13.4, 13.5), with the eye
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
/// same `pieces` give segments that line up one for one.
pub fn project_pieces(list: &[u8], eye: (f32, f32), out: &mut Vec<Primitive>, pieces: &mut Vec<u8>) -> Projection {
    let clamp = |v: f32| v.clamp(-MAX_EYE_OFFSET, MAX_EYE_OFFSET) as f64;
    let eye = (EYE_X + clamp(eye.0), EYE_Y + clamp(eye.1));
    let start = out.len();
    pieces.clear();
    let mut state = State::default();
    let (mut length, mut count) = (0.0f64, 0usize);
    let mut capped = false;
    let mut drawn = Vec::new();
    let mut faces = Vec::new();
    for c in decode(list) {
        let Some(figure) = state.step(&c) else { continue };
        let (n, len, f) = match &figure {
            Figure::Line(a, b) => (1, counted_length(*a, *b), 0),
            Figure::Point(_) => (1, 0.0, 0),
            Figure::Circle(ring) => (CIRCLE_CHORDS, (0..ring.len()).map(|k| counted_length(ring[k], ring[(k + 1) % ring.len()])).sum(), 0),
            Figure::Face(_) => (0, 0.0, 1),
        };
        let nf = faces.len() + f;
        if length + len > LENGTH_CAP || count + n > SEGMENT_CAP || nf > FACE_CAP || nf * (count + n) > FACE_WORK_CAP {
            capped = true;
            break;
        }
        length += len;
        count += n;
        match figure {
            Figure::Face(corners) => faces.push(corners),
            figure => drawn.push((figure, state.width.clamp(1, MAX_WIDTH) as u8, state.rgba)),
        }
    }
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
            Figure::Face(_) => unreachable!(),
        }
        if !faces.is_empty() {
            pieces.extend_from_slice(&((s.out.len() - before).min(u16::MAX as usize) as u16).to_le_bytes());
        }
    }
    capped |= s.capped;
    Projection { segments: out.len() - start, length, faces: faces.len(), capped }
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
        for unknown in [0x0D, 0x80, 0xFF] {
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
        // A horizontal line across the screen counts 640 units, whatever
        // part of it lies outside; 102 fit in 65,536, the 103rd does not,
        // and nothing after it is drawn.
        let across = [cmd(op::MOVE, &xyz(-16000, 1600, 0)), cmd(op::LINE, &xyz(26000, 1600, 0))].concat();
        let mut list: Vec<u8> = std::iter::repeat_n(across.clone(), 103).flatten().collect();
        list.extend(cmd(op::POINT, &[]));
        let mut out = Vec::new();
        let p = project(&list, (0.0, 0.0), &mut out);
        assert_eq!((p.segments, p.length, p.capped), (102, 102.0 * 640.0, true));
        // Counted with the reference eye: the same when the eye moves.
        let p = project(&list, (160.0, 0.0), &mut Vec::new());
        assert_eq!((p.segments, p.capped), (102, true));
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
        // 104 such circles fit by length (65,302), the 105th does not.
        let many: Vec<u8> = std::iter::repeat_n(circle.clone(), 106).flatten().collect();
        assert_eq!(project(&many, (0.0, 0.0), &mut Vec::new()).segments, 104 * 64);
        // By count: 256 circles of radius 0 are 16,384 segments; a point more is not drawn.
        let dot = [cmd(op::MOVE, &xyz(5120, 3840, 0)), cmd(op::CIRCLE, &[0, 0])].concat();
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
}
