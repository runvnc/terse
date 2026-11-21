//! Objective checks of the phosphor look (SPEC §vector.phosphor; the model
//! is in `phosphor.rs`), on pictures the software renderer (and the GPU
//! renderer, if this machine has an adapter) draws at 1280 × 960 and
//! 2560 × 1920 of purpose-made scenes:
//!
//! 1. text stays readable: in a grey "8 corners, 12 edges", the Michelson
//!    contrast between the glyphs' strokes and the gaps inside and between
//!    them (pixels the clean style leaves black) is at least 0.85;
//! 2. the strokes' peak is that of the clean style, within 3/255;
//! 3. a slanted line is sharp: the frequency at which its modulation
//!    transfer function (from its line spread function, measured across
//!    the line at a tenth of a pixel) falls to 50% is at least 80% of the
//!    clean style's;
//! 4. it glows: at least 15% of an isolated line's light lies more than
//!    3 pixels from it;
//! 5. overlaps add: a line drawn twice gives at least 1.4 times the light
//!    of one, within 1.5 pixels of it, after the tone curve;
//! 6. the core is about 1.2 pixels wide (full width at half maximum) at
//!    both sizes: it sharpens as the resolution grows.
//!
//! Light is measured linear (the sRGB values decoded), luminance as
//! 0.2126 R + 0.7152 G + 0.0722 B; the text checks use 8-bit luma
//! (0.299 R + 0.587 G + 0.114 B) as image tools do.

use terse_core::{Terminal, Vector};

use super::base::Pool;
use super::screen::SoftScreen;
use crate::display::{GlyphSource, VectorScene};
use crate::phosphor::{Look, Preset};

const SIZES: [(u32, u32); 2] = [(1280, 960), (2560, 1920)];

/// A picture: RGB bytes and its size.
struct Picture {
    size: (u32, u32),
    rgb: Vec<[u8; 3]>,
}

impl Picture {
    fn at(&self, x: i32, y: i32) -> [u8; 3] {
        self.rgb[(y as u32 * self.size.0 + x as u32) as usize]
    }
}

fn decode(v: u8) -> f64 {
    let c = v as f64 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Linear luminance of a pixel.
fn light(p: [u8; 3]) -> f64 {
    0.2126 * decode(p[0]) + 0.7152 * decode(p[1]) + 0.0722 * decode(p[2])
}

/// 8-bit luma of a pixel.
fn luma(p: [u8; 3]) -> f64 {
    0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64
}

/// The renderer drawing: the software renderer, or the GPU's.
enum Renderer {
    Soft,
    #[cfg(feature = "gpu")]
    Gpu(wgpu::Device, wgpu::Queue),
}

impl Renderer {
    /// The GPU renderer, if this machine has an adapter.
    #[cfg(feature = "gpu")]
    fn gpu() -> Option<Renderer> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        match pollster::block_on(crate::request_device(Default::default(), &instance)) {
            Ok((_, device, queue)) => Some(Renderer::Gpu(device, queue)),
            Err(e) => {
                eprintln!("phosphor look: no GPU ({e}); the GPU renderer is not checked");
                None
            }
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Renderer::Soft => "software",
            #[cfg(feature = "gpu")]
            Renderer::Gpu(..) => "GPU",
        }
    }

    /// The screen drawn with `terminal` and `list` in the clean or the
    /// phosphor style, after enough frames that the afterglow is steady.
    fn draw(&self, glyphs: Box<dyn GlyphSource>, size: (u32, u32), terminal: &Terminal, list: &[u8], phosphor: bool, look: Look) -> Picture {
        let mut v = Vector::default();
        v.submit(list);
        v.set_style(phosphor as u32);
        let scene = VectorScene { vector: Some(&v), vector3: None, eye: (0.0, 0.0), caps: crate::display::Caps::FULL };
        match self {
            Renderer::Soft => {
                let pool = Pool::new(4);
                let mut s = SoftScreen::new(glyphs);
                s.set_look(look);
                s.update(terminal);
                s.update_vectors(&scene);
                s.update_page(None, None);
                for _ in 0..40 {
                    s.accumulate(size, true, &pool);
                }
                s.render(size, &pool);
                assert_eq!(s.image.area.width() as u32, size.0);
                let rgb = s.image.data.iter().map(|&p| [(p >> 16) as u8, (p >> 8) as u8, p as u8]).collect();
                Picture { size, rgb }
            }
            #[cfg(feature = "gpu")]
            Renderer::Gpu(device, queue) => {
                use crate::display::Display;
                let format = wgpu::TextureFormat::Rgba8UnormSrgb;
                let mut r = crate::renderer::Renderer::new(&crate::renderer::Shaders::new(device), format, glyphs);
                r.set_look(look);
                r.update(queue, terminal);
                r.update_vectors(queue, &scene);
                r.update_page(None, None);
                for _ in 0..40 {
                    r.accumulate(device, queue, size);
                }
                let screens = crate::screens::Screens::new(r, format);
                let mut d = crate::screens::GpuDisplay::new(device.clone(), queue.clone(), screens, format, String::new());
                let rgba = d.snapshot(size).expect("a picture");
                Picture { size, rgb: rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2]]).collect() }
            }
        }
    }
}

/// Glyphs whose cells are as wide as the screen's at `size`.
fn glyphs(size: (u32, u32)) -> Option<Box<dyn GlyphSource>> {
    let g = crate::glyphs::SystemGlyphs::new(crate::FONT_PX, Some("DejaVu Sans Mono")).ok()?;
    Some(g.resized(size.0 / terse_core::COLS as u32))
}

/// No glyphs (the line scenes have no text).
struct NoGlyphs(u32);

impl GlyphSource for NoGlyphs {
    fn cell_size(&self) -> (u32, u32) {
        (self.0, self.0 * 2)
    }
    fn rasterize(&mut self, _: char, _: u32, _: &mut [u8]) {}
    fn resized(&self, width: u32) -> Box<dyn GlyphSource> {
        Box::new(NoGlyphs(width))
    }
}

fn blank() -> Terminal {
    let mut t = Terminal::default();
    t.command(0x06);
    t
}

/// The slanted test line, in sixteenths: from (160, 230) to (480, 262) units.
const LINE: [(f64, f64); 2] = [(160.0 * 16.0, 230.0 * 16.0), (480.0 * 16.0, 262.0 * 16.0)];

fn line_list(times: usize) -> Vec<u8> {
    let p = |v: f64| (v as i16).to_le_bytes();
    let mut list = Vec::new();
    for _ in 0..times {
        let [(x0, y0), (x1, y1)] = LINE;
        list.push(0x04);
        list.extend(p(x0));
        list.extend(p(y0));
        list.push(0x05);
        list.extend(p(x1));
        list.extend(p(y1));
    }
    list
}

/// The test line in pixels at `size`.
fn line_px(size: (u32, u32)) -> ((f64, f64), (f64, f64)) {
    let k = size.0 as f64 / (640.0 * 16.0);
    let [(x0, y0), (x1, y1)] = LINE;
    ((x0 * k, y0 * k), (x1 * k, y1 * k))
}

/// For each pixel: its signed distance from the line's axis, where along
/// it it lies (0 at the start, 1 at the end), and its distance from the
/// segment.
fn geometry(size: (u32, u32), x: i32, y: i32) -> (f64, f64, f64) {
    let ((x0, y0), (x1, y1)) = line_px(size);
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len = (dx * dx + dy * dy).sqrt();
    let (px, py) = (x as f64 + 0.5 - x0, y as f64 + 0.5 - y0);
    let t = (px * dx + py * dy) / (len * len);
    let d = (px * -dy + py * dx) / len;
    let h = t.clamp(0.0, 1.0);
    let (ex, ey) = (px - dx * h, py - dy * h);
    (d, t, (ex * ex + ey * ey).sqrt())
}

/// The line spread function across the line's middle 60%, at a tenth of a
/// pixel from -24 to 24 pixels.
fn spread(p: &Picture) -> Vec<f64> {
    const R: f64 = 24.0;
    let bins = (2.0 * R * 10.0) as usize + 1;
    let (mut sum, mut n) = (vec![0.0; bins], vec![0usize; bins]);
    let ((x0, y0), (x1, y1)) = line_px(p.size);
    for y in (y0.min(y1) - R) as i32..(y0.max(y1) + R) as i32 {
        for x in x0 as i32..x1 as i32 {
            let (d, t, _) = geometry(p.size, x, y);
            if !(0.2..=0.8).contains(&t) || d.abs() > R {
                continue;
            }
            let i = ((d + R) * 10.0).round() as usize;
            sum[i] += light(p.at(x, y));
            n[i] += 1;
        }
    }
    // Empty bins (none should be) take their neighbour's value.
    let mut out: Vec<f64> = sum.iter().zip(&n).map(|(s, &k)| if k > 0 { s / k as f64 } else { f64::NAN }).collect();
    for i in 1..out.len() {
        if out[i].is_nan() {
            out[i] = out[i - 1];
        }
    }
    out.iter().map(|v| if v.is_nan() { 0.0 } else { *v }).collect()
}

/// Full width at half maximum of a spread function sampled every 0.1 px.
fn fwhm(lsf: &[f64]) -> f64 {
    let (peak_i, &peak) = lsf.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap();
    let half = peak / 2.0;
    let cross = |range: Box<dyn Iterator<Item = usize>>| {
        let mut last = peak_i;
        for i in range {
            if lsf[i] < half {
                let t = (lsf[last] - half) / (lsf[last] - lsf[i]);
                return last as f64 + (i as f64 - last as f64) * t;
            }
            last = i;
        }
        last as f64
    };
    let right = cross(Box::new(peak_i + 1..lsf.len()));
    let left = cross(Box::new((0..peak_i).rev()));
    (right - left) * 0.1
}

/// The frequency (cycles a pixel) at which the transfer function of a
/// spread function sampled every 0.1 px first falls to 50%.
fn mtf50(lsf: &[f64]) -> f64 {
    let at = |f: f64| {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, v) in lsf.iter().enumerate() {
            let a = 2.0 * std::f64::consts::PI * f * i as f64 * 0.1;
            re += v * a.cos();
            im += v * a.sin();
        }
        (re * re + im * im).sqrt()
    };
    let dc = at(0.0);
    let mut last = (0.0, 1.0);
    for k in 1..=1000 {
        let f = k as f64 * 0.001;
        let m = at(f) / dc;
        if m < 0.5 {
            return last.0 + (f - last.0) * (last.1 - 0.5) / (last.1 - m);
        }
        last = (f, m);
    }
    1.0
}

/// Light more than 3 px from the segment as a share of all of it, and the
/// light within 1.5 px of the line's middle 60%.
fn shares(p: &Picture) -> (f64, f64) {
    let (mut far, mut total, mut core) = (0.0, 0.0, 0.0);
    for y in 0..p.size.1 as i32 {
        for x in 0..p.size.0 as i32 {
            let v = light(p.at(x, y));
            if v == 0.0 {
                continue;
            }
            let (d, t, seg) = geometry(p.size, x, y);
            total += v;
            if seg > 3.0 {
                far += v;
            }
            if (0.2..=0.8).contains(&t) && d.abs() <= 1.5 {
                core += v;
            }
        }
    }
    (far / total, core)
}

/// The text's row, column and colour.
const TEXT: (u32, u32, &str) = (20, 10, "8 corners, 12 edges");

fn text_terminal() -> Terminal {
    let mut t = blank();
    t.palette_set(9, &[150, 150, 150]);
    t.set_colours(9, 0);
    t.set_cursor(TEXT.1, TEXT.0);
    t.write(TEXT.2.as_bytes());
    t
}

/// Gap contrast and stroke peaks (phosphor, clean) of "12" in the text.
fn text_checks(clean: &Picture, ph: &Picture) -> (f64, f64, f64) {
    let (w, h) = clean.size;
    let (cw, ch) = (w / terse_core::COLS as u32, h / terse_core::ROWS as u32);
    let at = TEXT.2.find("12").unwrap() as u32;
    let (x0, x1) = ((TEXT.1 + at) * cw, (TEXT.1 + at + 2) * cw);
    let (y0, y1) = (TEXT.0 * ch, (TEXT.0 + 1) * ch);
    let clean_peak = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| luma(clean.at(x as i32, y as i32))).fold(0.0, f64::max);
    let ph_peak = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| luma(ph.at(x as i32, y as i32))).fold(0.0, f64::max);
    let (mut ink, mut gap) = (Vec::new(), Vec::new());
    for y in y0..y1 {
        for x in x0..x1 {
            let c = luma(clean.at(x as i32, y as i32));
            let v = luma(ph.at(x as i32, y as i32));
            if c >= 0.6 * clean_peak {
                ink.push(v);
            } else if c == 0.0 {
                // A gap: ink within 4 pixels on both sides in this row.
                let near = |dir: i32| (1..=4).any(|k| luma(clean.at(x as i32 + dir * k, y as i32)) >= 0.6 * clean_peak);
                if near(-1) && near(1) {
                    gap.push(v);
                }
            }
        }
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    let (i, g) = (mean(&ink), mean(&gap));
    ((i - g) / (i + g), ph_peak, clean_peak)
}

/// The six checks at both sizes, by both renderers; prints them, and
/// asserts them unless `TERSE_LOOK_REPORT` is set (to print the numbers of
/// any build).
#[test]
fn phosphor_look_checks() {
    let report = std::env::var_os("TERSE_LOOK_REPORT").is_some();
    #[cfg_attr(not(feature = "gpu"), allow(unused_mut))]
    let mut renderers = vec![Renderer::Soft];
    #[cfg(feature = "gpu")]
    renderers.extend(Renderer::gpu());
    let look = Look::default();
    for r in &renderers {
        for size in SIZES {
            let g = || -> Box<dyn GlyphSource> { Box::new(NoGlyphs(size.0 / terse_core::COLS as u32)) };
            let clean = r.draw(g(), size, &blank(), &line_list(1), false, look);
            let one = r.draw(g(), size, &blank(), &line_list(1), true, look);
            let two = r.draw(g(), size, &blank(), &line_list(2), true, look);
            let (lsf_clean, lsf) = (spread(&clean), spread(&one));
            let (sharp_clean, sharp) = (mtf50(&lsf_clean), mtf50(&lsf));
            let width = fwhm(&lsf);
            let (far, core_one) = shares(&one);
            let (_, core_two) = shares(&two);
            let overlap = core_two / core_one;
            let name = r.name();
            eprintln!(
                "phosphor look, {name}, {}x{}: MTF50 {sharp:.3} (clean {sharp_clean:.3}, ratio {:.2}); core FWHM {width:.2} px (clean {:.2}); \
                 light beyond 3 px {:.1}%; drawn twice {overlap:.2}x",
                size.0,
                size.1,
                sharp / sharp_clean,
                fwhm(&lsf_clean),
                far * 100.0
            );
            let text = glyphs(size).map(|gl| {
                let clean = r.draw(gl.resized(size.0 / terse_core::COLS as u32), size, &text_terminal(), &[], false, look);
                let ph = r.draw(gl, size, &text_terminal(), &[], true, look);
                let (contrast, peak, clean_peak) = text_checks(&clean, &ph);
                eprintln!("phosphor look, {name}, {}x{}: text gap contrast {contrast:.3}; stroke peak {peak:.0} (clean {clean_peak:.0})", size.0, size.1);
                (contrast, peak, clean_peak)
            });
            if report {
                continue;
            }
            assert!(sharp >= 0.8 * sharp_clean, "{name} {size:?}: MTF50 {sharp} against {sharp_clean}");
            assert!((1.0..=1.5).contains(&width), "{name} {size:?}: core FWHM {width}");
            assert!(far >= 0.15, "{name} {size:?}: halo {far}");
            assert!(overlap >= 1.4, "{name} {size:?}: drawn twice {overlap}");
            match text {
                Some((contrast, peak, clean_peak)) => {
                    assert!(contrast >= 0.85, "{name} {size:?}: gap contrast {contrast}");
                    assert!((peak - clean_peak).abs() <= 3.0, "{name} {size:?}: stroke peak {peak} against {clean_peak}");
                }
                None => eprintln!("phosphor look: DejaVu Sans Mono is not installed; the text is not checked"),
            }
        }
    }
}

/// The presets: a monochrome tube shows every colour in its own (a red
/// line on a green P31 tube is green), the colour tube each line in its
/// colour; the glass darkens the corners, not the centre; both renderers
/// agree.
#[test]
fn tube_presets_and_glass() {
    #[cfg_attr(not(feature = "gpu"), allow(unused_mut))]
    let mut renderers = vec![Renderer::Soft];
    #[cfg(feature = "gpu")]
    renderers.extend(Renderer::gpu());
    let size = (640, 480);
    let g = || -> Box<dyn GlyphSource> { Box::new(NoGlyphs(8)) };
    // A red line across the middle and one near the top left corner.
    let list = [
        &[0x01, 255, 0, 0, 255][..],
        &[0x04, 0, 0x14, 0, 0x0F, 0x05, 0, 0x1E, 0, 0x0F],
        &[0x04, 0x40, 0x01, 0x40, 0x01, 0x05, 0x00, 0x05, 0x40, 0x01],
    ]
    .concat();
    let mut pictures = Vec::new();
    for r in &renderers {
        let at = |p: &Picture, x: i32| (0..p.size.1 as i32).map(|y| p.at(x, y)).max_by_key(|c| c[0] as u32 + c[1] as u32 + c[2] as u32).unwrap();
        let xy = r.draw(g(), size, &blank(), &list, true, Look::default());
        let c = at(&xy, 400);
        assert!(c[0] > 150 && c[1] < 40 && c[2] < 40, "{}: colour tube {c:?}", r.name());
        let p31 = r.draw(g(), size, &blank(), &list, true, Look { preset: Preset::P31, ..Look::default() });
        let c = at(&p31, 400);
        assert!(c[1] > 120 && c[1] > c[0] + 40 && c[1] > c[2] + 40, "{}: P31 {c:?}", r.name());
        let amber = r.draw(g(), size, &blank(), &list, true, Look { preset: Preset::Amber, ..Look::default() });
        let c = at(&amber, 400);
        assert!(c[0] > c[1] && c[1] > c[2], "{}: amber {c:?}", r.name());
        let glass = r.draw(g(), size, &blank(), &list, true, Look { preset: Preset::Xy, vignette: true, ..Look::default() });
        let (centre, corner) = (at(&glass, 400), at(&glass, 60));
        let plain = at(&xy, 400);
        assert!((0..3).all(|i| centre[i].abs_diff(plain[i]) <= 1), "{}: the centre {centre:?} {plain:?}", r.name());
        assert!(corner[0] < at(&xy, 60)[0], "{}: the corner {corner:?}", r.name());
        pictures.push([xy, p31, amber, glass]);
    }
    if let [soft, gpu] = &pictures[..] {
        for (a, b) in soft.iter().zip(gpu) {
            let worst = a.rgb.iter().zip(&b.rgb).map(|(x, y)| (0..3).map(|i| x[i].abs_diff(y[i])).max().unwrap()).max().unwrap();
            assert!(worst <= 3, "GPU against software: {worst}");
        }
    }
}
