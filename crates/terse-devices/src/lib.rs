//! What the reference drivers share: premultiplied pixel arithmetic for
//! layers (DEVICES.md 7), light layers for the phosphor style (7.1), state restore helpers, and the page's quads drawn
//! into a layer. The drawing itself is terse-host's software renderer's
//! (glyphs, page layout, shapes); only the blending is the layers'.

use terse_host::soft::base::IRect;
use terse_host::soft::page::{Paint, PageQuads};

/// A premultiplied pixel: alpha in bits 24–31.
#[inline]
pub fn premul(r: u8, g: u8, b: u8, a: u8) -> u32 {
    let m = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u32;
    (a as u32) << 24 | m(r) << 16 | m(g) << 8 | m(b)
}

/// `src` (straight colour, 0–255 floats) with opacity `alpha` over a
/// premultiplied pixel, as `terse_host::soft::base::over` does for an
/// opaque target, with alpha as a fourth channel.
#[inline]
pub fn over4(dst: u32, src: [f32; 3], alpha: f32) -> u32 {
    let k = 1.0 - alpha;
    let ch = |s: f32, d: u32| (s * alpha + d as f32 * k + 0.5) as u32;
    ch(255.0, dst >> 24) << 24 | ch(src[0], (dst >> 16) & 255) << 16 | ch(src[1], (dst >> 8) & 255) << 8 | ch(src[2], dst & 255)
}

/// `mix` with a 0–255 coverage: the colour `rgb` with opacity `t` over a
/// premultiplied pixel.
#[inline]
pub fn mix4(dst: u32, rgb: u32, t: u32) -> u32 {
    let ch = |s: u32, d: u32| (d * (255 - t) + s * t + 127) / 255;
    ch(255, dst >> 24) << 24 | ch((rgb >> 16) & 255, (dst >> 16) & 255) << 16 | ch((rgb >> 8) & 255, (dst >> 8) & 255) << 8 | ch(rgb & 255, dst & 255)
}

/// Clears `r` of a layer `w` pixels wide to transparent.
pub fn clear(px: &mut [u32], w: usize, r: IRect) {
    for y in r.y0..r.y1 {
        px[y as usize * w + r.x0 as usize..y as usize * w + r.x1 as usize].fill(0);
    }
}

/// The page's quads (as `PageQuads::draw_clean` draws them) into a
/// premultiplied layer, within `run`.
pub fn draw_page(q: &PageQuads, px: &mut [u32], w: usize, run: IRect) {
    let clip = run.intersect(&q.scissor);
    if clip.is_empty() {
        return;
    }
    for quad in &q.quads {
        let r = quad.r.intersect(&clip);
        if r.is_empty() {
            continue;
        }
        for y in r.y0..r.y1 {
            let out = &mut px[y as usize * w + r.x0 as usize..y as usize * w + r.x1 as usize];
            match &quad.paint {
                Paint::Solid([cr, cg, cb, a]) => {
                    if *a == 0 {
                        continue;
                    }
                    let c = (*cr as u32) << 16 | (*cg as u32) << 8 | *cb as u32;
                    for p in out.iter_mut() {
                        *p = mix4(*p, c, *a as u32);
                    }
                }
                Paint::Mask(g, [cr, cg, cb]) => {
                    let c = (*cr as u32) << 16 | (*cg as u32) << 8 | *cb as u32;
                    let src = &g.data[(y - quad.r.y0) as usize * g.w as usize..][..g.w as usize];
                    let src = &src[(r.x0 - quad.r.x0) as usize..(r.x1 - quad.r.x0) as usize];
                    for (p, &a) in out.iter_mut().zip(src) {
                        if a > 0 {
                            *p = mix4(*p, c, a as u32);
                        }
                    }
                }
                Paint::Colour(g) => {
                    let src = &g.data[(y - quad.r.y0) as usize * g.w as usize * 4..][..g.w as usize * 4];
                    let src = &src[(r.x0 - quad.r.x0) as usize * 4..(r.x1 - quad.r.x0) as usize * 4];
                    for (p, t) in out.iter_mut().zip(src.chunks_exact(4)) {
                        if t[3] > 0 {
                            *p = over4(*p, [t[0] as f32, t[1] as f32, t[2] as f32], t[3] as f32 / 255.0);
                        }
                    }
                }
                Paint::Figure(f) => {
                    let (qw, qh) = (quad.r.width() as f32, quad.r.height() as f32);
                    let v = (((y - quad.r.y0) as f32 + 0.5) / qh * f.h as f32) as usize;
                    let src = &f.data[v.min(f.h as usize - 1) * f.w as usize * 4..][..f.w as usize * 4];
                    for (i, p) in out.iter_mut().enumerate() {
                        let x = r.x0 + i as i32;
                        let u = (((x - quad.r.x0) as f32 + 0.5) / qw * f.w as f32) as usize;
                        let t = &src[u.min(f.w as usize - 1) * 4..][..4];
                        if t[3] > 0 {
                            let a = t[3] as f32 / 255.0;
                            let k = 1.0 / a.max(1.0 / 255.0);
                            *p = over4(*p, [t[0] as f32 * k, t[1] as f32 * k, t[2] as f32 * k], a);
                        }
                    }
                }
            }
        }
    }
}

/// Linear light of an sRGB value (0–1).
fn to_linear(c: f32) -> f32 {
    terse_host::soft::phosphor::to_linear(c)
}

/// sRGB of linear light (0–1), as a byte.
fn to_srgb8(c: f32) -> u32 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u32
}

/// Adds light (straight colour `rgb`, coverage `a`) to a pixel of a light
/// layer (DEVICES.md 7.1): where the pixel holds none it is that light
/// exactly; where it holds some, the two are added (linear, the coverage
/// at most 255), as the reference host's tube adds the page's light.
#[inline]
fn add_light(dst: &mut u32, rgb: [u8; 3], a: u8) {
    if a == 0 {
        return;
    }
    let da = *dst >> 24;
    if da == 0 {
        *dst = (a as u32) << 24 | (rgb[0] as u32) << 16 | (rgb[1] as u32) << 8 | rgb[2] as u32;
        return;
    }
    let total = (da + a as u32).min(255);
    let ch = |d: u32, s: u8| {
        let l = (to_linear(d as f32 / 255.0) * da as f32 + to_linear(s as f32 / 255.0) * a as f32) / total as f32;
        to_srgb8(l)
    };
    let p = *dst;
    *dst = total << 24 | ch(p >> 16 & 255, rgb[0]) << 16 | ch(p >> 8 & 255, rgb[1]) << 8 | ch(p & 255, rgb[2]);
}

/// The page's quads (prepared as light: `SoftPage::prepare` with
/// `phosphor`) into a light layer (DEVICES.md 7.1), within `run`: each
/// pixel's light as terse-host's tube takes it (`PageQuads::draw_light`),
/// colour straight and coverage in the alpha.
pub fn draw_page_light(q: &PageQuads, px: &mut [u32], w: usize, run: IRect) {
    let clip = run.intersect(&q.scissor);
    if clip.is_empty() {
        return;
    }
    for quad in &q.quads {
        let r = quad.r.intersect(&clip);
        if r.is_empty() {
            continue;
        }
        for y in r.y0..r.y1 {
            let out = &mut px[y as usize * w + r.x0 as usize..y as usize * w + r.x1 as usize];
            match &quad.paint {
                Paint::Solid([cr, cg, cb, a]) => {
                    for p in out.iter_mut() {
                        add_light(p, [*cr, *cg, *cb], *a);
                    }
                }
                Paint::Mask(g, rgb) => {
                    let src = &g.data[(y - quad.r.y0) as usize * g.w as usize..][..g.w as usize];
                    let src = &src[(r.x0 - quad.r.x0) as usize..(r.x1 - quad.r.x0) as usize];
                    for (p, &a) in out.iter_mut().zip(src) {
                        add_light(p, *rgb, a);
                    }
                }
                Paint::Colour(g) => {
                    let src = &g.data[(y - quad.r.y0) as usize * g.w as usize * 4..][..g.w as usize * 4];
                    let src = &src[(r.x0 - quad.r.x0) as usize * 4..(r.x1 - quad.r.x0) as usize * 4];
                    for (p, t) in out.iter_mut().zip(src.chunks_exact(4)) {
                        add_light(p, [t[0], t[1], t[2]], t[3]);
                    }
                }
                Paint::Figure(f) => {
                    let (qw, qh) = (quad.r.width() as f32, quad.r.height() as f32);
                    let v = (((y - quad.r.y0) as f32 + 0.5) / qh * f.h as f32) as usize;
                    let src = &f.data[v.min(f.h as usize - 1) * f.w as usize * 4..][..f.w as usize * 4];
                    for (i, p) in out.iter_mut().enumerate() {
                        let x = r.x0 + i as i32;
                        let u = (((x - quad.r.x0) as f32 + 0.5) / qw * f.w as f32) as usize;
                        let t = &src[u.min(f.w as usize - 1) * 4..][..4];
                        if t[3] > 0 {
                            // Premultiplied: the colour is the value over the coverage.
                            let k = 255.0 / t[3] as f32;
                            let c = |v: u8| (v as f32 * k + 0.5).min(255.0) as u8;
                            add_light(p, [c(t[0]), c(t[1]), c(t[2])], t[3]);
                        }
                    }
                }
            }
        }
    }
}

/// The picture's pixels covered by the screen rectangle, clipped.
pub fn scissor(screen: (f32, f32, f32, f32), picture: (u32, u32)) -> IRect {
    IRect::covering(screen).intersect(&IRect::of_size(picture))
}

/// An IRect as damage.
pub fn rect(r: IRect) -> [i32; 4] {
    [r.x0, r.y0, r.x1, r.y1]
}

/// The union of two rectangles (an empty one counts for nothing).
pub fn union(a: IRect, b: IRect) -> IRect {
    if a.is_empty() {
        return b;
    }
    if b.is_empty() {
        return a;
    }
    IRect::new(a.x0.min(b.x0), a.y0.min(b.y0), a.x1.max(b.x1), a.y1.max(b.y1))
}
