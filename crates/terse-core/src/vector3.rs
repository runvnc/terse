//! Device 9, 3D vector display (SPEC 13).
//!
//! The device only validates and copies the display list; the style and
//! persistence it sets are device 3's ([`crate::Vector`]), shared by both
//! displays. [`decode`] turns a stored list into its commands (SPEC 13.3
//! termination rules, the transform stack's limits), and [`project`] turns
//! a list and an eye into the line segments a host draws on the glass, with
//! the caps of SPEC 13.4 and the depth cue of SPEC 13.5. Every renderer
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
        out.push_str(&format!("-- 3d: {} segments, length {:.1} units{} --\n", p.segments, p.length, if p.capped { ", capped" } else { "" }));
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
        }
    }
}

/// Operand bytes of each opcode, or `None` for End and unknown opcodes.
fn operand_len(opcode: u8) -> Option<usize> {
    Some(match opcode {
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
        let Some(n) = operand_len(opcode) else { return end(self) };
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
/// or a circle as its chord polygon (the points at k/64 of a turn).
#[derive(Clone, Debug, PartialEq)]
pub enum Figure {
    Line([f64; 3], [f64; 3]),
    Point([f64; 3]),
    Circle(Vec<[f64; 3]>),
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

/// What a list drew, seen from the reference eye.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Projection {
    /// Segments drawn (`out`'s length).
    pub segments: usize,
    /// Their counted length (SPEC 13.4), in screen units.
    pub length: f64,
    /// A command was stopped by a cap.
    pub capped: bool,
}

/// The segments a host draws for `list` (SPEC 13.4, 13.5), with the eye
/// moved `eye` (screen units, each at most [`MAX_EYE_OFFSET`]) from the
/// reference eye, as [`Primitive::Beam`]s in sixteenths on the glass, in
/// list order, appended to `out`. Lines and points have round ends; a
/// circle's chords join with butt ends (`butt`). Each end carries its depth
/// cue (`fade`, 255 = 1). Segments are clipped to the near plane and to
/// the screen grown by a margin. What the caps count is measured with the
/// reference eye, whatever `eye` is.
pub fn project(list: &[u8], eye: (f32, f32), out: &mut Vec<Primitive>) -> Projection {
    let clamp = |v: f32| v.clamp(-MAX_EYE_OFFSET, MAX_EYE_OFFSET) as f64;
    let eye = (EYE_X + clamp(eye.0), EYE_Y + clamp(eye.1));
    let start = out.len();
    let mut state = State::default();
    let (mut length, mut count) = (0.0f64, 0usize);
    let mut capped = false;
    for c in decode(list) {
        let Some(figure) = state.step(&c) else { continue };
        let (n, len) = match &figure {
            Figure::Line(a, b) => (1, counted_length(*a, *b)),
            Figure::Point(_) => (1, 0.0),
            Figure::Circle(ring) => (CIRCLE_CHORDS, (0..ring.len()).map(|k| counted_length(ring[k], ring[(k + 1) % ring.len()])).sum()),
        };
        if length + len > LENGTH_CAP || count + n > SEGMENT_CAP {
            capped = true;
            break;
        }
        length += len;
        count += n;
        let width = state.width.clamp(1, MAX_WIDTH) as u8;
        let rgba = state.rgba;
        match figure {
            Figure::Line(a, b) => segment(a, b, 0, width, rgba, eye, out),
            Figure::Point(p) => segment(p, p, 0, width, rgba, eye, out),
            Figure::Circle(ring) => {
                for k in 0..ring.len() {
                    segment(ring[k], ring[(k + 1) % ring.len()], BUTT_START | BUTT_END, width, rgba, eye, out);
                }
            }
        }
    }
    Projection { segments: out.len() - start, length, capped }
}

/// The segment a–b (screen units) seen from `eye`, if any of it is drawn.
fn segment(a: [f64; 3], b: [f64; 3], butt: u8, width: u8, rgba: Rgba, eye: (f64, f64), out: &mut Vec<Primitive>) {
    let Some((a, b)) = near_clip(a, b) else { return };
    let (p, q) = (to_glass(a, eye), to_glass(b, eye));
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
    let ((x0, y0, g0), (x1, y1, g1)) = (at(t0), at(t1));
    out.push(Primitive::Beam { x0, y0, x1, y1, width, rgba, fade: [g0, g1], butt });
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
        for unknown in [0x0C, 0x80, 0xFF] {
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
}
