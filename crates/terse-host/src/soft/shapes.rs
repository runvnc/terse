//! The vector display's primitives on the CPU (SPEC §vector.list), drawn the way
//! the GPU renderer's shader draws them (`vectors.rs`): lines, circle
//! outlines and discs by their distance from the pixel's centre, with
//! round ends and a one-pixel antialiased edge; rectangles and triangles as
//! plain geometry (pixel centres inside). In the phosphor style a line or
//! circle gives light instead of coverage: the Gaussian beam of
//! `phosphor::beam_light`, with σ in pixels, and the vertex dwell
//! (`phosphor.rs`).
//!
//! Each shape is visited row by row over only the pixels it can reach (the
//! row's interval of a capsule, ring or disc), so a long thin line costs
//! its length, not its bounding box.

use terse_core::{Primitive, SCREEN_HEIGHT, SCREEN_WIDTH};

use super::base::IRect;
use crate::phosphor::{BEAM_REACH, DOT, DWELL, erf, peak, sigma};

/// How sixteenths of a screen unit map to the pixels of a target: pixel
/// position = origin + sixteenths × scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geom {
    pub origin: (f32, f32),
    pub scale: (f32, f32),
}

impl Geom {
    /// The screen drawn into `rect` (x, y, w, h) pixels.
    pub fn for_rect(rect: (f32, f32, f32, f32)) -> Self {
        let (x, y, w, h) = rect;
        Geom { origin: (x, y), scale: (w / (SCREEN_WIDTH as f32 * 16.0), h / (SCREEN_HEIGHT as f32 * 16.0)) }
    }

    /// One pixel in sixteenths (the mean of the two axes).
    fn px(&self) -> f32 {
        2.0 / (self.scale.0 + self.scale.1)
    }
}

/// A primitive ready to draw: coordinates in sixteenths (f32), colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// A line, or a 3D segment: `fade` is its depth cue at each end (1 for
    /// a 2D line), `butt` its butt ends (`terse_core::vector3::BUTT_START`, `BUTT_END`).
    Line { a: (f32, f32), b: (f32, f32), hw: f32, rgba: [u8; 4], dwell: u32, fade: (f32, f32), butt: u8 },
    Circle { c: (f32, f32), r: f32, hw: f32, rgba: [u8; 4] },
    Disc { c: (f32, f32), r: f32, rgba: [u8; 4] },
    Rect { a: (f32, f32), b: (f32, f32), rgba: [u8; 4] },
    Triangle { p: [(f32, f32); 3], rgba: [u8; 4] },
}

impl Shape {
    /// `p` as drawn; `dwell` is its vertex dwell (`phosphor::dwell_ends`, 0
    /// for none).
    pub fn new(p: &Primitive, dwell: u32) -> Self {
        let f = |x: i16, y: i16| (x as f32, y as f32);
        match *p {
            Primitive::Line { x0, y0, x1, y1, width, rgba } => {
                Shape::Line { a: f(x0, y0), b: f(x1, y1), hw: width as f32 * 0.5, rgba, dwell, fade: (1.0, 1.0), butt: 0 }
            }
            Primitive::Beam { x0, y0, x1, y1, width, rgba, fade, butt } => Shape::Line {
                a: f(x0, y0),
                b: f(x1, y1),
                hw: width as f32 * 0.5,
                rgba,
                dwell,
                fade: (fade[0] as f32 / 255.0, fade[1] as f32 / 255.0),
                butt,
            },
            Primitive::Circle { x, y, radius, width, rgba } => {
                Shape::Circle { c: f(x, y), r: radius as f32, hw: width as f32 * 0.5, rgba }
            }
            Primitive::Disc { x, y, radius, rgba } => Shape::Disc { c: f(x, y), r: radius as f32, rgba },
            Primitive::Rect { x0, y0, x1, y1, rgba } => Shape::Rect { a: f(x0, y0), b: f(x1, y1), rgba },
            Primitive::Triangle { x0, y0, x1, y1, x2, y2, rgba } => {
                Shape::Triangle { p: [f(x0, y0), f(x1, y1), f(x2, y2)], rgba }
            }
        }
    }

    pub fn rgba(&self) -> [u8; 4] {
        match *self {
            Shape::Line { rgba, .. }
            | Shape::Circle { rgba, .. }
            | Shape::Disc { rgba, .. }
            | Shape::Rect { rgba, .. }
            | Shape::Triangle { rgba, .. } => rgba,
        }
    }

    /// How far from its centre line (or centre) the shape reaches, in
    /// sixteenths, beyond its own extent: the antialiased edge, or with
    /// `glow` the phosphor beam's Gaussian.
    fn reach(&self, g: &Geom, glow: bool) -> f32 {
        let px = g.px();
        match *self {
            Shape::Line { hw, .. } | Shape::Circle { hw, .. } => {
                if glow {
                    BEAM_REACH * sigma(2.0 * hw, px) * px
                } else {
                    hw + px
                }
            }
            Shape::Disc { .. } => px,
            Shape::Rect { .. } | Shape::Triangle { .. } => 0.0,
        }
    }

    /// The pixels the shape may touch (conservative).
    pub fn bounds(&self, g: &Geom, glow: bool) -> IRect {
        let reach = self.reach(g, glow);
        let (lo, hi) = match *self {
            Shape::Line { a, b, .. } => ((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0), a.1.max(b.1))),
            Shape::Circle { c, r, .. } | Shape::Disc { c, r, .. } => ((c.0 - r, c.1 - r), (c.0 + r, c.1 + r)),
            Shape::Rect { a, b, .. } => ((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0), a.1.max(b.1))),
            Shape::Triangle { p, .. } => (
                (p[0].0.min(p[1].0).min(p[2].0), p[0].1.min(p[1].1).min(p[2].1)),
                (p[0].0.max(p[1].0).max(p[2].0), p[0].1.max(p[1].1).max(p[2].1)),
            ),
        };
        let x0 = g.origin.0 + (lo.0 - reach) * g.scale.0;
        let y0 = g.origin.1 + (lo.1 - reach) * g.scale.1;
        let x1 = g.origin.0 + (hi.0 + reach) * g.scale.0;
        let y1 = g.origin.1 + (hi.1 + reach) * g.scale.1;
        IRect::new(x0.floor() as i32 - 1, y0.floor() as i32 - 1, x1.ceil() as i32 + 1, y1.ceil() as i32 + 1)
    }
}

/// How far inside the half-pixel edge (in pixels) a pixel centre must lie
/// for `visit` to take it as fully covered without measuring it: far more
/// than the rounding of the distances.
const INTERIOR_MARGIN: f32 = 0.01;

/// Coverage of a pixel whose centre is d pixels from the centre line of a
/// stroke hw pixels wide on each side; strokes thinner than a pixel fade
/// instead of thinning.
#[inline]
fn stroke(hw: f32, d: f32) -> f32 {
    if hw >= 0.5 { (hw + 0.5 - d).clamp(0.0, 1.0) } else { (1.0 - d).clamp(0.0, 1.0) * 2.0 * hw }
}

#[cfg(test)]
fn segment_distance(q: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let pa = (q.0 - a.0, q.1 - a.1);
    let ba = (b.0 - a.0, b.1 - a.1);
    let h = ((pa.0 * ba.0 + pa.1 * ba.1) / (ba.0 * ba.0 + ba.1 * ba.1).max(1e-6)).clamp(0.0, 1.0);
    let (dx, dy) = (pa.0 - ba.0 * h, pa.1 - ba.1 * h);
    (dx * dx + dy * dy).sqrt()
}

/// The interval of x where the disc of radius `r` at `c` meets the line
/// y = `y`, if it does.
#[inline]
fn disc_row(c: (f32, f32), r: f32, y: f32) -> Option<(f32, f32)> {
    let dy = y - c.1;
    let k = r * r - dy * dy;
    (k >= 0.0).then(|| {
        let s = k.sqrt();
        (c.0 - s, c.0 + s)
    })
}

/// The interval of x where the capsule (segment a–b, radius r) meets the
/// line y = `y`: the extremes of its two end discs and its two sides.
fn capsule_row(a: (f32, f32), b: (f32, f32), r: f32, y: f32) -> Option<(f32, f32)> {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for c in [a, b] {
        if let Some((l, h)) = disc_row(c, r, y) {
            lo = lo.min(l);
            hi = hi.max(h);
        }
    }
    let d = (b.0 - a.0, b.1 - a.1);
    let len = (d.0 * d.0 + d.1 * d.1).sqrt();
    if len > 1e-6 && d.1.abs() > 1e-9 {
        let n = (-d.1 / len * r, d.0 / len * r);
        for s in [1.0, -1.0] {
            // Side a + s n → b + s n crosses y at t.
            let t = (y - (a.1 + s * n.1)) / d.1;
            if (0.0..=1.0).contains(&t) {
                let x = a.0 + s * n.0 + d.0 * t;
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
    } else if len > 1e-6 && (y - a.1).abs() <= r {
        // Horizontal: the band between the end discs.
        lo = lo.min(a.0.min(b.0));
        hi = hi.max(a.0.max(b.0));
    }
    (lo <= hi).then_some((lo, hi))
}

/// What a visit computes per pixel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Coverage 0–1 (the clean style).
    Cover,
    /// Phosphor light (lines and circles only), before colour.
    Light,
}

/// Calls `f(x, y, value)` for each pixel of `clip` the shape gives
/// something to, in rows then columns; `value` is its coverage or light.
pub fn visit(shape: &Shape, g: &Geom, mode: Mode, clip: IRect, mut f: impl FnMut(i32, i32, f32)) {
    let glow = mode == Mode::Light;
    let clip = clip.intersect(&shape.bounds(g, glow));
    if clip.is_empty() {
        return;
    }
    let px = g.px();
    let reach = shape.reach(g, glow);
    // Sixteenths of a pixel's centre (by multiplying: no division per
    // pixel), and back.
    let (inv_sx, inv_sy, inv_px) = (1.0 / g.scale.0, 1.0 / g.scale.1, 1.0 / px);
    let qx = |x: i32| (x as f32 + 0.5 - g.origin.0) * inv_sx;
    let qy = |y: i32| (y as f32 + 0.5 - g.origin.1) * inv_sy;
    // Pixels whose centres lie in [lo, hi] sixteenths (one spare on each side).
    let cols = |lo: f32, hi: f32| {
        let x0 = (g.origin.0 + lo * g.scale.0 - 0.5).floor() as i32 - 1;
        let x1 = (g.origin.0 + hi * g.scale.0 - 0.5).ceil() as i32 + 2;
        (x0.max(clip.x0), x1.min(clip.x1))
    };
    // The pixels whose centres lie in [lo, hi] sixteenths (none for `None`).
    let inside = |span: Option<(f32, f32)>| match span {
        Some((lo, hi)) => ((g.origin.0 + lo * g.scale.0 - 0.5).ceil() as i32, (g.origin.0 + hi * g.scale.0 - 0.5).floor() as i32 + 1),
        None => (0, 0),
    };
    match *shape {
        Shape::Line { a, b, hw, dwell, fade, butt, .. } => {
            let beam = glow.then(|| Beam::new(a, b, hw, dwell, fade, px));
            let ba = (b.0 - a.0, b.1 - a.1);
            let len2 = ba.0 * ba.0 + ba.1 * ba.1;
            let inv_len2 = 1.0 / len2.max(1e-6);
            let hw_px = hw * inv_px;
            let plain = butt == 0 && fade == (1.0, 1.0);
            // A butt end: a step half a pixel either side of it instead of
            // a round cap (as the GPU's `segment`).
            let (b0, b1) = (butt & 1 != 0, butt & 2 != 0);
            let len_px = len2.sqrt() * inv_px;
            // The interior: pixel centres within this distance of the
            // segment are fully covered (`stroke` is 1 there, with a margin
            // for rounding), so their coverage is the depth cue alone. Not
            // for butt ends, whose coverage also depends on the position
            // along the line.
            let inner = (beam.is_none() && butt == 0 && hw_px >= 0.5).then(|| (hw_px - 0.5 - INTERIOR_MARGIN) * px).filter(|&r| r > 0.0);
            for y in clip.y0..clip.y1 {
                let yq = qy(y);
                let Some((lo, hi)) = capsule_row(a, b, reach, yq) else { continue };
                let (x0, x1) = cols(lo, hi);
                let pay = yq - a.1;
                let (in0, in1) = inside(inner.and_then(|r| capsule_row(a, b, r, yq)));
                for x in x0..x1 {
                    let q = (qx(x), yq);
                    if x >= in0 && x < in1 {
                        let w = if plain {
                            1.0
                        } else {
                            let h = ((q.0 - a.0) * ba.0 + pay * ba.1) * inv_len2;
                            fade.0 + (fade.1 - fade.0) * h.clamp(0.0, 1.0)
                        };
                        if w > 0.0 {
                            f(x, y, w);
                        }
                        continue;
                    }
                    if let Some(beam) = &beam {
                        let v = beam.at(q);
                        if v > 0.0 {
                            f(x, y, v);
                        }
                        continue;
                    }
                    // The distance to the segment.
                    let pax = q.0 - a.0;
                    let hr = (pax * ba.0 + pay * ba.1) * inv_len2;
                    let mut h = hr.clamp(0.0, 1.0);
                    let mut w = 1.0;
                    if !plain {
                        w = fade.0 + (fade.1 - fade.0) * h;
                        if butt != 0 {
                            let along = hr * len_px;
                            let s0 = if b0 { (along + 0.5).clamp(0.0, 1.0) } else { 1.0 };
                            let s1 = if b1 { (along - len_px + 0.5).clamp(0.0, 1.0) } else { 0.0 };
                            w *= s0 - s1;
                            if (b0 && hr < 0.0) || (b1 && hr > 1.0) {
                                h = hr;
                            }
                        }
                    }
                    let (dx, dy) = (pax - ba.0 * h, pay - ba.1 * h);
                    let d = (dx * dx + dy * dy).sqrt();
                    let v = stroke(hw_px, d * inv_px) * w;
                    if v > 0.0 {
                        f(x, y, v);
                    }
                }
            }
        }
        Shape::Circle { c, r, hw, .. } => {
            let hw_px = hw * inv_px;
            let inv_s = 1.0 / (sigma(2.0 * hw, px) * px);
            let top = peak(2.0 * hw);
            let (outer, inner) = (r + reach, r - reach);
            for y in clip.y0..clip.y1 {
                let yq = qy(y);
                let Some((lo, hi)) = disc_row(c, outer, yq) else { continue };
                // The hole, if the row crosses it.
                let hole = if inner > 0.0 { disc_row(c, inner, yq) } else { None };
                let spans = match hole {
                    Some((hl, hh)) => [(lo, hl), (hh, hi)],
                    None => [(lo, hi), (1.0, 0.0)],
                };
                for (lo, hi) in spans {
                    if lo > hi {
                        continue;
                    }
                    let (x0, x1) = cols(lo, hi);
                    for x in x0..x1 {
                        let q = (qx(x), yq);
                        let d = (((q.0 - c.0).powi(2) + (q.1 - c.1).powi(2)).sqrt() - r).abs();
                        let v = match mode {
                            Mode::Cover => stroke(hw_px, d * inv_px),
                            Mode::Light => {
                                let k = d * inv_s;
                                top * gauss(k * k)
                            }
                        };
                        if v > 0.0 {
                            f(x, y, v);
                        }
                    }
                }
            }
        }
        Shape::Disc { c, r, .. } => {
            if mode == Mode::Light {
                return;
            }
            // The interior, fully covered (as for a line).
            let inner = r - (0.5 + INTERIOR_MARGIN) * px;
            for y in clip.y0..clip.y1 {
                let yq = qy(y);
                let Some((lo, hi)) = disc_row(c, r + reach, yq) else { continue };
                let (x0, x1) = cols(lo, hi);
                let (in0, in1) = inside((inner > 0.0).then(|| disc_row(c, inner, yq)).flatten());
                for x in x0..x1 {
                    if x >= in0 && x < in1 {
                        f(x, y, 1.0);
                        continue;
                    }
                    let q = (qx(x), yq);
                    let len = ((q.0 - c.0).powi(2) + (q.1 - c.1).powi(2)).sqrt();
                    let v = ((r - len) * inv_px + 0.5).clamp(0.0, 1.0);
                    if v > 0.0 {
                        f(x, y, v);
                    }
                }
            }
        }
        Shape::Rect { a, b, .. } => {
            if mode == Mode::Light {
                return;
            }
            // Pixel centres inside [min, max).
            let p0 = (g.origin.0 + a.0.min(b.0) * g.scale.0, g.origin.1 + a.1.min(b.1) * g.scale.1);
            let p1 = (g.origin.0 + a.0.max(b.0) * g.scale.0, g.origin.1 + a.1.max(b.1) * g.scale.1);
            let first = |v: f32| (v - 0.5).ceil() as i32;
            let r = clip.intersect(&IRect::new(first(p0.0), first(p0.1), first(p1.0), first(p1.1)));
            for y in r.y0..r.y1 {
                for x in r.x0..r.x1 {
                    f(x, y, 1.0);
                }
            }
        }
        Shape::Triangle { p, .. } => {
            if mode == Mode::Light {
                return;
            }
            let v = p.map(|(x, y)| (g.origin.0 + x * g.scale.0, g.origin.1 + y * g.scale.1));
            let area = (v[1].0 - v[0].0) * (v[2].1 - v[0].1) - (v[2].0 - v[0].0) * (v[1].1 - v[0].1);
            if area.abs() < 1e-6 {
                return;
            }
            for y in clip.y0..clip.y1 {
                let yc = y as f32 + 0.5;
                // Where the row crosses the edges (half-open in y, so a
                // vertex on the row counts once).
                let mut lo = f32::INFINITY;
                let mut hi = f32::NEG_INFINITY;
                for i in 0..3 {
                    let (s, e) = (v[i], v[(i + 1) % 3]);
                    let (top, bottom) = if s.1 <= e.1 { (s, e) } else { (e, s) };
                    if yc >= top.1 && yc < bottom.1 {
                        let x = top.0 + (yc - top.1) / (bottom.1 - top.1) * (bottom.0 - top.0);
                        lo = lo.min(x);
                        hi = hi.max(x);
                    }
                }
                if lo > hi {
                    continue;
                }
                let x0 = ((lo - 0.5).ceil() as i32).max(clip.x0);
                let x1 = ((hi - 0.5).ceil() as i32).min(clip.x1);
                for x in x0..x1 {
                    f(x, y, 1.0);
                }
            }
        }
    }
}

/// Steps of the tables of `gauss` (over `GAUSS_END`) and `erf_table`
/// (over ±`ERF_END`): linear between them, within 10⁻⁵ of the functions.
const STEPS: usize = 4096;
const GAUSS_END: f32 = 24.0;
const ERF_END: f32 = 6.0;

struct Tables {
    gauss: Vec<f32>,
    erf: Vec<f32>,
}

fn tables() -> &'static Tables {
    static TABLES: std::sync::OnceLock<Tables> = std::sync::OnceLock::new();
    TABLES.get_or_init(|| Tables {
        gauss: (0..=STEPS + 1).map(|i| (-0.5 * (i as f32 * GAUSS_END / STEPS as f32)).exp()).collect(),
        erf: (0..=STEPS + 1).map(|i| erf(i as f32 * 2.0 * ERF_END / STEPS as f32 - ERF_END)).collect(),
    })
}

/// exp(−s / 2), from the table (0 beyond its end: 6·10⁻⁶).
#[inline]
fn gauss(s: f32) -> f32 {
    let f = s * (STEPS as f32 / GAUSS_END);
    if f >= STEPS as f32 {
        return 0.0;
    }
    let t = tables();
    let i = f as usize;
    t.gauss[i] + (t.gauss[i + 1] - t.gauss[i]) * (f - i as f32)
}

/// The error function, from the table (±1 beyond it).
#[inline]
fn erf_table(x: f32) -> f32 {
    let f = (x.clamp(-ERF_END, ERF_END) + ERF_END) * (STEPS as f32 / (2.0 * ERF_END));
    let t = tables();
    let i = (f as usize).min(STEPS - 1);
    t.erf[i] + (t.erf[i + 1] - t.erf[i]) * (f - i as f32)
}

/// `phosphor::beam_light` from the tables.
#[inline]
fn light(along: f32, across: f32, len: f32) -> f32 {
    let g = gauss(across * across);
    if g == 0.0 {
        return 0.0;
    }
    if len <= 1e-4 {
        return DOT * g * gauss(along * along);
    }
    if along > 5.5 && along < len - 5.5 {
        return g;
    }
    const S: f32 = std::f32::consts::FRAC_1_SQRT_2;
    g * 0.5 * (erf_table(along * S) - erf_table((along - len) * S))
}

/// A line's phosphor light (`phosphor::beam_light`): its geometry in units
/// of the beam's σ, its peak, its depth cue and its vertex dwell.
struct Beam {
    a: (f32, f32),
    b: (f32, f32),
    dir: (f32, f32),
    /// Length in σ, and 1 / σ (σ in sixteenths).
    len: f32,
    inv_s: f32,
    peak: f32,
    fade: (f32, f32),
    /// The dwell spot's light at each end, relative to the peak (0: none).
    dwell: (f32, f32),
}

impl Beam {
    fn new(a: (f32, f32), b: (f32, f32), hw: f32, dwell: u32, fade: (f32, f32), px: f32) -> Self {
        let s = sigma(2.0 * hw, px) * px;
        let d = (b.0 - a.0, b.1 - a.1);
        let len = (d.0 * d.0 + d.1 * d.1).sqrt();
        let dir = if len > 1e-3 { (d.0 / len, d.1 / len) } else { (1.0, 0.0) };
        // An end whose count is 0 (a butt end) has none; a dot one at a.
        let k = |n: u32| if n == 0 { 0.0 } else { DWELL * DOT / n as f32 };
        let k1 = if a != b { k((dwell >> 4) & 15) } else { 0.0 };
        Beam { a, b, dir, len: len / s, inv_s: 1.0 / s, peak: peak(2.0 * hw), fade, dwell: (k(dwell & 15), k1) }
    }

    #[inline]
    fn at(&self, q: (f32, f32)) -> f32 {
        let (rx, ry) = ((q.0 - self.a.0) * self.inv_s, (q.1 - self.a.1) * self.inv_s);
        let along = rx * self.dir.0 + ry * self.dir.1;
        let across = ry * self.dir.0 - rx * self.dir.1;
        let mut v = light(along, across, self.len);
        // The dwell spots, where they are more than 10⁻¹³ of the peak.
        let r2 = rx * rx + ry * ry;
        if self.dwell.0 != 0.0 && r2 < 60.0 {
            v += self.dwell.0 * gauss(r2);
        }
        if self.dwell.1 != 0.0 {
            let (ex, ey) = ((q.0 - self.b.0) * self.inv_s, (q.1 - self.b.1) * self.inv_s);
            let e2 = ex * ex + ey * ey;
            if e2 < 60.0 {
                v += self.dwell.1 * gauss(e2);
            }
        }
        let h = if self.len > 0.0 { (along / self.len).clamp(0.0, 1.0) } else { 0.0 };
        v * self.peak * (self.fade.0 + (self.fade.1 - self.fade.0) * h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covered(shape: &Shape, g: &Geom, mode: Mode, clip: IRect) -> Vec<(i32, i32, f32)> {
        let mut out = Vec::new();
        visit(shape, g, mode, clip, |x, y, v| out.push((x, y, v)));
        out
    }

    /// One pixel per screen unit: 1/16 pixel per sixteenth.
    fn unit() -> Geom {
        Geom { origin: (0.0, 0.0), scale: (1.0 / 16.0, 1.0 / 16.0) }
    }

    #[test]
    fn line_rows_cover_the_capsule_exactly() {
        // Brute force over the bounding box against the spans.
        let g = Geom { origin: (3.0, 5.0), scale: (0.137, 0.137) };
        let clip = IRect::new(0, 0, 400, 400);
        for (a, b, hw) in [((100.0, 200.0), (900.0, 1300.0), 8.0), ((50.0, 50.0), (50.0, 50.0), 24.0), ((10.0, 300.0), (800.0, 300.0), 3.0)] {
            for mode in [Mode::Cover, Mode::Light] {
                let shape = Shape::Line { a, b, hw, rgba: [255; 4], dwell: 0x11, fade: (1.0, 1.0), butt: 0 };
                let got = covered(&shape, &g, mode, clip);
                let mut want: Vec<(i32, i32, f32)> = Vec::new();
                let px = g.px();
                let bounds = shape.bounds(&g, mode == Mode::Light).intersect(&clip);
                for y in bounds.y0..bounds.y1 {
                    for x in bounds.x0..bounds.x1 {
                        let q = ((x as f32 + 0.5 - g.origin.0) / g.scale.0, (y as f32 + 0.5 - g.origin.1) / g.scale.1);
                        let d = segment_distance(q, a, b);
                        let v = match mode {
                            Mode::Cover => stroke(hw / px, d / px),
                            Mode::Light => Beam::new(a, b, hw, 0x11, (1.0, 1.0), px).at(q),
                        };
                        if v > 1e-4 {
                            want.push((x, y, v));
                        }
                    }
                }
                // The same pixels (but for vanishing ones), within rounding.
                let got: Vec<_> = got.into_iter().filter(|p| p.2 > 1e-4).collect();
                assert_eq!(got.len(), want.len(), "{a:?} {b:?} {hw}");
                for (g, w) in got.iter().zip(&want) {
                    assert!(g.0 == w.0 && g.1 == w.1 && (g.2 - w.2).abs() < 1e-4, "{g:?} {w:?}");
                }
            }
        }
    }

    /// Butt ends: a line cut into pieces that join with butt ends covers
    /// what the whole line does (no bright beads where they join, as a
    /// circle's chords do), and the depth cue fades it along its length.
    #[test]
    fn butt_ends_join_seamlessly() {
        let g = Geom { origin: (0.5, 0.25), scale: (0.4, 0.4) };
        let clip = IRect::new(0, 0, 200, 60);
        let line = |a, b, butt, fade| Shape::Line { a, b, hw: 12.0, rgba: [255; 4], dwell: 0, fade, butt };
        for mode in [Mode::Cover, Mode::Light] {
            let mut whole = vec![0.0f32; 200 * 60];
            visit(&line((40.0, 70.0), (440.0, 70.0), 0, (1.0, 1.0)), &g, mode, clip, |x, y, v| whole[(y * 200 + x) as usize] += v);
            let mut pieces = vec![0.0f32; 200 * 60];
            let cuts = [40.0, 77.3, 150.0, 151.0, 300.5, 440.0];
            for i in 0..cuts.len() - 1 {
                let butt = (if i > 0 { 1 } else { 0 }) | (if i + 2 < cuts.len() { 2 } else { 0 });
                let s = line((cuts[i], 70.0), (cuts[i + 1], 70.0), butt, (1.0, 1.0));
                visit(&s, &g, mode, clip, |x, y, v| pieces[(y * 200 + x) as usize] += v);
            }
            let worst = whole.iter().zip(&pieces).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            let at = whole.iter().zip(&pieces).position(|(a, b)| (a - b).abs() == worst).unwrap();
            assert!(worst < 1e-3, "{worst} at {} {} {:?}: {} {}", at % 200, at / 200, mode == Mode::Cover, whole[at], pieces[at]);
            // Faded from 1 to 0.5: the far end has half the light.
            let mut faded = vec![0.0f32; 200 * 60];
            visit(&line((40.0, 70.0), (440.0, 70.0), 0, (1.0, 0.5)), &g, mode, clip, |x, y, v| faded[(y * 200 + x) as usize] += v);
            let (near, far) = ((28 * 200 + 25) as usize, (28 * 200 + 170) as usize);
            assert!((faded[near] / whole[near] - 0.97).abs() < 0.02, "{}", faded[near] / whole[near]);
            assert!((faded[far] / whole[far] - 0.519).abs() < 0.01, "{}", faded[far] / whole[far]);
        }
        // A zero-length piece with butt ends draws nothing.
        let mut n = 0;
        visit(&line((80.0, 70.0), (80.0, 70.0), 3, (1.0, 1.0)), &g, Mode::Cover, clip, |_, _, _| n += 1);
        assert_eq!(n, 0);
    }

    /// The interiors taken as covered without measuring are exactly what
    /// measuring gives: a disc, and a faded line, against the formulas
    /// pixel by pixel.
    #[test]
    fn interiors_are_exact() {
        let clip = IRect::new(0, 0, 300, 300);
        for g in [Geom { origin: (3.0, 5.0), scale: (0.137, 0.137) }, Geom { origin: (0.0, 0.0), scale: (0.0859, 0.0859) }] {
            let px = g.px();
            let disc = Shape::Disc { c: (1000.3, 1200.7), r: 640.0, rgba: [255; 4] };
            let mut n = 0;
            for (x, y, v) in covered(&disc, &g, Mode::Cover, clip) {
                let q = ((x as f32 + 0.5 - g.origin.0) / g.scale.0, (y as f32 + 0.5 - g.origin.1) / g.scale.1);
                let len = ((q.0 - 1000.3f32).powi(2) + (q.1 - 1200.7f32).powi(2)).sqrt();
                let want = ((640.0 - len) / px + 0.5).clamp(0.0, 1.0);
                assert!((v - want).abs() < 2e-4, "disc {x} {y}: {v} {want}");
                n += (v == 1.0) as usize;
            }
            assert!(n > 1000);
            for fade in [(1.0, 1.0), (0.9, 0.4)] {
                let (a, b, hw) = ((150.0, 210.0), (1900.0, 1400.0), 120.0);
                let line = Shape::Line { a, b, hw, rgba: [255; 4], dwell: 0, fade, butt: 0 };
                for (x, y, v) in covered(&line, &g, Mode::Cover, clip) {
                    let q = ((x as f32 + 0.5 - g.origin.0) / g.scale.0, (y as f32 + 0.5 - g.origin.1) / g.scale.1);
                    let ba = (b.0 - a.0, b.1 - a.1);
                    let h = (((q.0 - a.0) * ba.0 + (q.1 - a.1) * ba.1) / (ba.0 * ba.0 + ba.1 * ba.1)).clamp(0.0, 1.0);
                    let want = stroke(hw / px, segment_distance(q, a, b) / px) * (fade.0 + (fade.1 - fade.0) * h);
                    assert!((v - want).abs() < 2e-4, "line {x} {y}: {v} {want}");
                }
            }
        }
    }

    /// The tables follow the functions they stand for.
    #[test]
    fn the_tables_follow_the_beam() {
        for i in 0..20000 {
            let x = i as f32 * 0.0007 - 7.0;
            assert!((erf_table(x) - erf(x)).abs() < 2e-5, "erf {x}");
            let s = i as f32 * 0.0013;
            assert!((gauss(s) - (-0.5 * s).exp()).abs() < 1e-5, "gauss {s}");
            for len in [0.0, 3.0, 40.0] {
                let (along, across) = (x * 1.3 + 2.0, x * 0.4);
                let want = crate::phosphor::beam_light(along, across, len);
                assert!((light(along, across, len) - want).abs() < 3e-5, "{along} {across} {len}");
            }
        }
    }

    #[test]
    fn rectangles_and_triangles_take_pixel_centres() {
        let g = unit();
        // 2.0 to 5.0 units: pixel centres 2.5, 3.5, 4.5.
        let r = Shape::Rect { a: (32.0, 32.0), b: (80.0, 48.0), rgba: [255; 4] };
        let got: Vec<(i32, i32)> = covered(&r, &g, Mode::Cover, IRect::new(0, 0, 10, 10)).iter().map(|&(x, y, _)| (x, y)).collect();
        assert_eq!(got, vec![(2, 2), (3, 2), (4, 2)]);
        // A right triangle with legs of 4 pixels covers about half the square.
        let t = Shape::Triangle { p: [(0.0, 0.0), (64.0, 0.0), (0.0, 64.0)], rgba: [255; 4] };
        let n = covered(&t, &g, Mode::Cover, IRect::new(0, 0, 10, 10)).len();
        assert!((6..=10).contains(&n), "{n}");
        // Clipped.
        assert!(covered(&r, &g, Mode::Cover, IRect::new(0, 0, 3, 3)).len() == 1);
    }

    #[test]
    fn discs_and_rings() {
        let g = unit();
        let disc = Shape::Disc { c: (160.0, 160.0), r: 80.0, rgba: [255; 4] };
        let cover: f32 = covered(&disc, &g, Mode::Cover, IRect::new(0, 0, 40, 40)).iter().map(|v| v.2).sum();
        assert!((cover - std::f32::consts::PI * 25.0).abs() < 1.5, "{cover}");
        let ring = Shape::Circle { c: (320.0, 320.0), r: 160.0, hw: 8.0, rgba: [255; 4] };
        let cover: f32 = covered(&ring, &g, Mode::Cover, IRect::new(0, 0, 60, 60)).iter().map(|v| v.2).sum();
        let want = 2.0 * std::f32::consts::PI * 10.0;
        assert!((cover - want).abs() < 2.0, "{cover} {want}");
        // No pixel at the centre.
        assert!(covered(&ring, &g, Mode::Cover, IRect::new(20, 20, 21, 21)).is_empty());
    }
}
