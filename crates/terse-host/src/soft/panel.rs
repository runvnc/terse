//! Panels in the software renderer (SPEC 12.3, 13.3 Screen): a machine's
//! picture drawn in perspective onto the shell's screen. The machine is
//! drawn into an image of its own (`SoftScreen` with a rectangle at the
//! origin); a pyramid of that image, each level half the one before,
//! averaged in linear light, is sampled per pixel: the ray from the eye
//! through the pixel's centre meets the panel's plane at (u, v), the
//! footprint of the pixel there picks two levels, and each is sampled
//! bilinearly (trilinear filtering), so text on a slowly turning panel
//! does not shimmer. The panel's edges are antialiased by their distance
//! from the pixel's centre. Seen from behind, the panel is a dark plate.
//!
//! With `PanelMode::Flat` (`--panels flat`), the permitted fallback of a
//! host that cannot draw in perspective, the picture is drawn upright,
//! fitted into the bounding rectangle of the panel's projection.

use terse_core::vector3::{NEAR_Z, Panel};

use super::base::{IRect, Pixels, pack, unpack};
use crate::display::{PANEL_BACK, to_pixels};

/// How panels are drawn (`--panels`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelMode {
    Perspective,
    Flat,
}

/// One level of a pyramid: its size and pixels (XRGB).
struct Level {
    w: usize,
    h: usize,
    px: Vec<u32>,
}

/// A machine's picture and its smaller copies.
pub struct Pyramid {
    levels: Vec<Level>,
}

/// Steps of linear light in the tables.
const LIN_STEPS: u32 = 16_383;

/// sRGB (8 bits) to linear light in `LIN_STEPS`, and back.
struct Tables {
    lin: [u16; 256],
    srgb: Vec<u8>,
}

fn tables() -> &'static Tables {
    static T: std::sync::OnceLock<Tables> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let to_lin = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        let to_srgb = |c: f32| if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
        let n = LIN_STEPS as f32;
        Tables {
            lin: std::array::from_fn(|i| (to_lin(i as f32 / 255.0) * n + 0.5) as u16),
            srgb: (0..=LIN_STEPS).map(|i| (to_srgb(i as f32 / n) * 255.0 + 0.5) as u8).collect(),
        }
    })
}

impl Pyramid {
    /// The pyramid of `img` (its whole area).
    pub fn build(img: &Pixels) -> Pyramid {
        let t = tables();
        let (w, h) = (img.area.width(), img.area.height());
        let mut levels = vec![Level { w, h, px: img.data.clone() }];
        while let Some(l) = levels.last().filter(|l| l.w > 1 || l.h > 1) {
            let (nw, nh) = (l.w.div_ceil(2), l.h.div_ceil(2));
            let mut px = Vec::with_capacity(nw * nh);
            for y in 0..nh {
                let (y0, y1) = (2 * y, (2 * y + 1).min(l.h - 1));
                for x in 0..nw {
                    let (x0, x1) = (2 * x, (2 * x + 1).min(l.w - 1));
                    let mut sum = [0u32; 3];
                    for p in [l.px[y0 * l.w + x0], l.px[y0 * l.w + x1], l.px[y1 * l.w + x0], l.px[y1 * l.w + x1]] {
                        let c = unpack(p);
                        for i in 0..3 {
                            sum[i] += t.lin[c[i] as usize] as u32;
                        }
                    }
                    let c = sum.map(|v| t.srgb[((v + 2) / 4) as usize]);
                    px.push(pack(c[0], c[1], c[2]));
                }
            }
            levels.push(Level { w: nw, h: nh, px });
        }
        Pyramid { levels }
    }

    /// The size of the full picture.
    pub fn size(&self) -> (usize, usize) {
        (self.levels[0].w, self.levels[0].h)
    }
}

/// A panel as the pixels of a target see it: the visible polygon (in
/// front of the near plane) in pixels, and the map from a pixel to (u, v)
/// on the panel.
pub struct Placed {
    poly: Vec<(f64, f64)>,
    /// The eye, in screen units.
    eye: [f64; 3],
    /// Panel corner 0 and axes (screen units), the plane's normal.
    c0: [f64; 3],
    ax: [f64; 3],
    ay: [f64; 3],
    n: [f64; 3],
    /// The target's screen rectangle: pixels per unit, origin.
    rect: (f64, f64, f64),
    front: bool,
    /// The part of the pyramid's picture the panel shows: u0, v0, and its
    /// width and height, as fractions of the picture (all of it for a
    /// Screen).
    window: [f32; 4],
    /// Its source's width over its height (4/3 for a Screen).
    shape: f32,
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

impl Placed {
    /// `p` seen from the eye at `eye` (screen units) on a screen drawn into
    /// `rect` (pixels).
    pub fn new(p: &Panel, eye: (f64, f64), rect: (f32, f32, f32, f32)) -> Placed {
        // The part in front of the near plane (a little inside it).
        let near = NEAR_Z + 1e-3;
        let mut poly3 = Vec::with_capacity(5);
        for k in 0..4 {
            let (a, b) = (p.corners[k], p.corners[(k + 1) % 4]);
            if a[2] > near {
                poly3.push(a);
            }
            if (a[2] > near) != (b[2] > near) {
                let t = (near - a[2]) / (b[2] - a[2]);
                poly3.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, near]);
            }
        }
        let poly = poly3.iter().map(|&c| {
            let (x, y, _) = to_pixels(c, eye, rect);
            (x, y)
        });
        let c = &p.corners;
        let (ax, ay) = (sub(c[1], c[0]), sub(c[3], c[0]));
        Placed {
            poly: poly.collect(),
            eye: [eye.0, eye.1, -terse_core::vector3::EYE_DISTANCE],
            c0: c[0],
            ax,
            ay,
            n: cross(ax, ay),
            rect: (rect.2 as f64 / terse_core::SCREEN_WIDTH as f64, rect.0 as f64, rect.1 as f64),
            front: p.front,
            window: [0.0, 0.0, 1.0, 1.0],
            shape: p.source[2] as f32 / p.source[3].max(1) as f32,
        }
    }

    /// The panel showing the part `window` (u0, v0, width, height, as
    /// fractions) of its pyramid's picture.
    pub fn with_window(self, window: [f32; 4]) -> Placed {
        Placed { window, ..self }
    }

    /// The pixels the panel touches.
    pub fn bounds(&self) -> IRect {
        if self.poly.len() < 3 {
            return IRect::EMPTY;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &(x, y) in &self.poly {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let big = 1e6;
        IRect::new(x0.clamp(-big, big).floor() as i32, y0.clamp(-big, big).floor() as i32, x1.clamp(-big, big).ceil() as i32 + 1, y1.clamp(-big, big).ceil() as i32 + 1)
    }

    /// (u, v) on the panel and their rates per pixel (du/dx, dv/dx,
    /// du/dy, dv/dy) at pixel position (x, y).
    #[inline]
    fn uv(&self, x: f64, y: f64) -> Option<(f64, f64, [f64; 4])> {
        let (s, ox, oy) = self.rect;
        let g = [(x - ox) / s, (y - oy) / s, 0.0];
        let d = sub(g, self.eye);
        let den = dot(self.n, d);
        if den.abs() < 1e-12 {
            return None;
        }
        let k = dot(self.n, sub(self.c0, self.eye));
        let t = k / den;
        let e = sub(self.eye, self.c0);
        let (aa, bb) = (dot(self.ax, self.ax), dot(self.ay, self.ay));
        let (dax, day) = (dot(d, self.ax), dot(d, self.ay));
        let u = (dot(e, self.ax) + t * dax) / aa;
        let v = (dot(e, self.ay) + t * day) / bb;
        // d u / d g = k (ax den − dax n) / (aa den²), and per pixel 1 / s.
        let r = k / (den * den * s);
        let du = |i: usize| r * (self.ax[i] * den - dax * self.n[i]) / aa;
        let dv = |i: usize| r * (self.ay[i] * den - day * self.n[i]) / bb;
        Some((u, v, [du(0), dv(0), du(1), dv(1)]))
    }

    /// The level of detail at pixel position (x, y) for a picture of
    /// `size`: log2 of the larger footprint of a pixel, in texels.
    fn lod(&self, x: f64, y: f64, size: (usize, usize)) -> f32 {
        let Some((_, _, d)) = self.uv(x, y) else { return 0.0 };
        let (w, h) = (size.0 as f64 * self.window[2] as f64, size.1 as f64 * self.window[3] as f64);
        let rx = ((d[0] * w).powi(2) + (d[1] * h).powi(2)).sqrt();
        let ry = ((d[2] * w).powi(2) + (d[3] * h).powi(2)).sqrt();
        let rho = rx.max(ry);
        if rho > 1.0 { rho.log2() as f32 } else { 0.0 }
    }

    /// The polygon's edges as (a, b, c) with a x + b y + c the distance
    /// (pixels) inside it.
    fn edges(&self) -> Vec<[f64; 3]> {
        let n = self.poly.len();
        let area: f64 = (0..n)
            .map(|i| {
                let (p, q) = (self.poly[i], self.poly[(i + 1) % n]);
                p.0 * q.1 - q.0 * p.1
            })
            .sum();
        let sign = if area >= 0.0 { 1.0 } else { -1.0 };
        (0..n)
            .filter_map(|i| {
                let (p, q) = (self.poly[i], self.poly[(i + 1) % n]);
                let (dx, dy) = (q.0 - p.0, q.1 - p.1);
                let l = (dx * dx + dy * dy).sqrt();
                (l > 1e-9).then(|| {
                    let (a, b) = (-sign * dy / l, sign * dx / l);
                    [a, b, -(a * p.0 + b * p.1)]
                })
            })
            .collect()
    }

    /// Flat (the fallback): where the picture is drawn upright, scaled
    /// to fit the bounding rectangle of the projection and centred in it
    /// (as SPEC 12.3 fits a picture): (x, y, w, h) in pixels.
    fn fitted(&self) -> (f32, f32, f32, f32) {
        let b = self.bounds();
        let (w, h) = (b.width().saturating_sub(1).max(1) as f32, b.height().saturating_sub(1).max(1) as f32);
        let k = self.shape;
        let (bw, bh) = if w <= h * k { (w, w / k) } else { (h * k, h) };
        (b.x0 as f32 + (w - bw) / 2.0, b.y0 as f32 + (h - bh) / 2.0, bw, bh)
    }
}

/// `src` over `dst` by `w` (0–256), per channel.
#[inline]
fn lerp_px(a: u32, b: u32, w: u32) -> u32 {
    let iw = 256 - w;
    let rb = (((a & 0xFF00FF) * iw + (b & 0xFF00FF) * w) >> 8) & 0xFF00FF;
    let g = (((a & 0xFF00) * iw + (b & 0xFF00) * w) >> 8) & 0xFF00;
    rb | g
}

/// Pixels along a row between two level-of-detail estimates.
const LOD_SPAN: i32 = 16;

/// The pixels x0..x1 of a row (centres at x + 0.5, y + 0.5) at which every
/// edge's distance is at least `at`: an interval, empty if lo > hi.
fn span(edges: &[[f64; 3]], cy: f64, x0: i32, x1: i32, at: f64) -> (i32, i32) {
    let (mut lo, mut hi) = (x0 as f64, (x1 - 1) as f64);
    for &[a, b, c] in edges {
        // a (x + 0.5) + b cy + c ≥ at.
        let k = at - b * cy - c - a * 0.5;
        if a.abs() < 1e-12 {
            if k > 0.0 {
                return (1, 0);
            }
        } else if a > 0.0 {
            lo = lo.max((k / a).ceil());
        } else {
            hi = hi.min((k / a).floor());
        }
    }
    if lo > hi { (1, 0) } else { (lo as i32, hi as i32) }
}

/// One or two levels of a pyramid, and how much of the second: what a
/// stretch of pixels samples (trilinear filtering, the level chosen for
/// the stretch).
struct Levels<'a> {
    a: &'a Level,
    b: Option<(&'a Level, u32)>,
}

impl<'a> Levels<'a> {
    fn new(p: &'a Pyramid, lod: f32) -> Self {
        let top = p.levels.len() - 1;
        let lod = lod.clamp(0.0, top as f32);
        let k = lod as usize;
        let f = ((lod - k as f32) * 256.0) as u32;
        // Within 1/16 of a level: that level alone.
        if k == top || f < 16 {
            Levels { a: &p.levels[k], b: None }
        } else if f >= 240 {
            Levels { a: &p.levels[k + 1], b: None }
        } else {
            Levels { a: &p.levels[k], b: Some((&p.levels[k + 1], f)) }
        }
    }

    #[inline(always)]
    fn at(&self, u: f32, v: f32) -> u32 {
        let c = bilinear(self.a, u, v);
        match self.b {
            None => c,
            Some((l, f)) => lerp_px(c, bilinear(l, u, v), f),
        }
    }
}

/// `l` at (u, v), bilinear, clamped at the edges; in 1/256 texel steps.
#[inline(always)]
fn bilinear(l: &Level, u: f32, v: f32) -> u32 {
    let (w, h) = (l.w as i32, l.h as i32);
    let x = ((u * (w * 256) as f32) as i32 - 128).clamp(0, (w - 1) * 256);
    let y = ((v * (h * 256) as f32) as i32 - 128).clamp(0, (h - 1) * 256);
    let (x0, y0) = (x >> 8, y >> 8);
    let (fx, fy) = ((x & 255) as u32, (y & 255) as u32);
    let dx = (x0 + 1 < w) as usize;
    let dy = if y0 + 1 < h { l.w } else { 0 };
    let i = y0 as usize * l.w + x0 as usize;
    let Some(px) = l.px.get(i..i + dx + dy + 1) else { return 0 };
    let top = lerp_px(px[0], px[dx], fx);
    let bottom = lerp_px(px[dy], px[dy + dx], fx);
    lerp_px(top, bottom, fy)
}

/// Draws `placed` (far to near) into the rows of `band` (pixels of `area`,
/// from row `y0`) within `clip`: each panel from its pyramid (none yet, or
/// seen from behind: the dark plate), with its edges antialiased; a pixel
/// a nearer panel covers wholly is not drawn for a farther one.
pub fn draw_panels(placed: &[(Placed, Option<&Pyramid>)], band: &mut [u32], area: IRect, y0: i32, clip: IRect, mode: PanelMode) {
    let stride = area.width();
    let back = pack(PANEL_BACK[0], PANEL_BACK[1], PANEL_BACK[2]);
    if mode == PanelMode::Flat {
        // Each picture in its fitted rectangle, at one level, where no
        // nearer one covers it.
        let fitted: Vec<IRect> = placed
            .iter()
            .map(|(p, _)| {
                let (x, y, w, h) = p.fitted();
                IRect::new(x.round() as i32, y.round() as i32, (x + w).round() as i32, (y + h).round() as i32)
            })
            .collect();
        for (i, (p, pyramid)) in placed.iter().enumerate() {
            let r = fitted[i].intersect(&clip);
            if r.is_empty() {
                continue;
            }
            let (fx, fy, fw, fh) = p.fitted();
            let [u0, v0, du, dv] = p.window;
            let levels = pyramid.filter(|_| p.front).map(|pyr| Levels::new(pyr, (pyr.size().0 as f32 * du / fw).max(1.0).log2()));
            for y in r.y0..r.y1 {
                let row = &mut band[(y - y0) as usize * stride..][..stride];
                let v = v0 + dv * (y as f32 + 0.5 - fy) / fh;
                for x in r.x0..r.x1 {
                    if fitted[i + 1..].iter().any(|n| n.x0 <= x && x < n.x1 && n.y0 <= y && y < n.y1) {
                        continue;
                    }
                    row[(x - area.x0) as usize] = match &levels {
                        Some(l) => l.at(u0 + du * (x as f32 + 0.5 - fx) / fw, v),
                        None => back,
                    };
                }
            }
        }
        return;
    }
    let edges: Vec<Vec<[f64; 3]>> = placed.iter().map(|(p, _)| p.edges()).collect();
    let bounds: Vec<IRect> = placed.iter().map(|(p, _)| p.bounds().intersect(&clip)).collect();
    let mut runs: Vec<(i32, i32)> = Vec::with_capacity(8);
    for y in clip.y0..clip.y1 {
        let row = &mut band[(y - y0) as usize * stride..][..stride];
        let cy = y as f64 + 0.5;
        for (i, (p, pyramid)) in placed.iter().enumerate() {
            let b = bounds[i];
            if y < b.y0 || y >= b.y1 || b.is_empty() || edges[i].len() < 3 {
                continue;
            }
            let (xs, xe) = span(&edges[i], cy, b.x0, b.x1, -0.5 + 1e-9);
            if xs > xe {
                continue;
            }
            let (fs, fe) = span(&edges[i], cy, b.x0, b.x1, 0.5);
            // What is left of xs..=xe where no nearer panel covers the row
            // wholly.
            runs.clear();
            runs.push((xs, xe));
            for j in i + 1..placed.len() {
                let bj = bounds[j];
                if y < bj.y0 || y >= bj.y1 || bj.is_empty() || edges[j].len() < 3 {
                    continue;
                }
                let (cs, ce) = span(&edges[j], cy, xs, xe + 1, 0.5);
                if cs > ce {
                    continue;
                }
                let mut next = Vec::with_capacity(runs.len() + 1);
                for &(a, z) in &runs {
                    if ce < a || cs > z {
                        next.push((a, z));
                        continue;
                    }
                    if a < cs {
                        next.push((a, cs - 1));
                    }
                    if ce < z {
                        next.push((ce + 1, z));
                    }
                }
                runs = next;
            }
            let (s, ox, oy) = p.rect;
            let k = dot(p.n, sub(p.c0, p.eye));
            let e = sub(p.eye, p.c0);
            let (aa, bb) = (dot(p.ax, p.ax), dot(p.ay, p.ay));
            let (pu, pv) = (dot(e, p.ax), dot(e, p.ay));
            // u = (pu den + k (d · ax)) / (aa den), v likewise, with den =
            // n · d and d linear in x: numerators and denominator by steps.
            let d0 = sub([(xs as f64 + 0.5 - ox) / s, (cy - oy) / s, 0.0], p.eye);
            let lin = |w: [f64; 3]| (dot(w, d0), w[0] / s);
            let (den0, sden) = lin(p.n);
            let (dax0, sdax) = lin(p.ax);
            let (day0, sday) = lin(p.ay);
            let (nu0, snu) = ((pu * den0 + k * dax0) / aa, (pu * sden + k * sdax) / aa);
            let (nv0, snv) = ((pv * den0 + k * day0) / bb, (pv * sden + k * sday) / bb);
            let sample = pyramid.filter(|_| p.front);
            let [u0, v0, du, dv] = p.window.map(|v| v as f64);
            let cover = |x: i32| -> u32 {
                if x >= fs && x <= fe {
                    return 256;
                }
                let cx = x as f64 + 0.5;
                let m = edges[i].iter().map(|&[a, b, c]| a * cx + b * cy + c).fold(f64::MAX, f64::min) + 0.5;
                (m.clamp(0.0, 1.0) * 256.0) as u32
            };
            for &(r0, r1) in &runs {
                let mut x = r0;
                while x <= r1 {
                    let end = (x + LOD_SPAN - 1).min(r1);
                    let levels = sample.map(|pyr| Levels::new(pyr, p.lod(x as f64 + 0.5, cy, pyr.size())));
                    for x in x..=end {
                        let w = cover(x);
                        if w == 0 {
                            continue;
                        }
                        let c = match &levels {
                            Some(l) => {
                                let t = (x - xs) as f64;
                                let den = den0 + sden * t;
                                if den.abs() < 1e-12 {
                                    continue;
                                }
                                let r = 1.0 / den;
                                l.at((u0 + du * (nu0 + snu * t) * r) as f32, (v0 + dv * (nv0 + snv * t) * r) as f32)
                            }
                            None => back,
                        };
                        let px = &mut row[(x - area.x0) as usize];
                        *px = if w >= 256 { c } else { lerp_px(*px, c, w) };
                    }
                    x = end + 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pyramid_levels_average() {
        let mut img = Pixels::new(IRect::new(0, 0, 4, 2));
        img.data = vec![pack(255, 0, 0), pack(255, 0, 0), pack(0, 0, 0), pack(0, 0, 0), pack(255, 0, 0), pack(255, 0, 0), pack(0, 0, 0), pack(0, 0, 0)];
        let p = Pyramid::build(&img);
        assert_eq!(p.levels.iter().map(|l| (l.w, l.h)).collect::<Vec<_>>(), [(4, 2), (2, 1), (1, 1)]);
        assert_eq!(unpack(p.levels[1].px[0]), [255, 0, 0]);
        // Half red, half black, in linear light: about 188, not 128.
        assert_eq!(unpack(p.levels[2].px[0])[0], 188);
    }
}
