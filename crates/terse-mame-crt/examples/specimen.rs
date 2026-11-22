//! Specimen sheets of the stroke fonts, drawn on the CPU (a soft beam whose
//! light adds where strokes meet, roughly as the tube shows it), for
//! designing glyphs without a graphics adapter:
//!
//!     cargo run -p terse-mame-crt --example specimen -- <out dir>
//!
//! Writes, for each font, `specimen-<font>.png` (every glyph of the basic
//! repertoire, 6 pixels a unit, with the arcade grid's points faint behind
//! the first row) and `prose-<font>-<n>x.png` (a paragraph at 1×, 2×, 3×,
//! enlarged without smoothing to the same size).

use terse_mame_crt::stroke::{CELL_H, CELL_W, Font, Stroke, strokes_in};

const PROSE: &[&str] = &[
    "A vector monitor draws only what is there: the beam moves from point to",
    "point and lights the phosphor where it passes, so a letter is a handful",
    "of strokes and the screen between the letters stays black. Quick brown",
    "foxes jump over 0O 1lI 5S 8B 2Z uv rn/m c/e il1| {[()]} \"quoted\", 3.14;",
    "café, naïve, Ærøskøbing, Straße, señor, garçon: 12 °C, ½ £, ¿qué? ÄÖÜ É.",
];

struct Canvas {
    w: usize,
    h: usize,
    light: Vec<f32>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Canvas { w, h, light: vec![0.0; w * h] }
    }

    /// A stroke at `scale` pixels a unit, offset (ox, oy) in units.
    fn stroke(&mut self, s: &Stroke, scale: f32, ox: f32, oy: f32, gain: f32) {
        let (x0, y0, x1, y1) = ((s.x0 + ox) * scale, (s.y0 + oy) * scale, (s.x1 + ox) * scale, (s.y1 + oy) * scale);
        let sigma = 0.45 * scale * s.width;
        let r = 3.0 * sigma + 1.0;
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len2 = (dx * dx + dy * dy).max(1e-6);
        let (xa, xb) = ((x0.min(x1) - r).max(0.0) as usize, ((x0.max(x1) + r) as usize).min(self.w - 1));
        let (ya, yb) = ((y0.min(y1) - r).max(0.0) as usize, ((y0.max(y1) + r) as usize).min(self.h - 1));
        for py in ya..=yb {
            for px in xa..=xb {
                let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
                let t = (((x - x0) * dx + (y - y0) * dy) / len2).clamp(0.0, 1.0);
                let d2 = (x0 + t * dx - x).powi(2) + (y0 + t * dy - y).powi(2);
                self.light[py * self.w + px] += gain * s.level * (-d2 / (2.0 * sigma * sigma)).exp();
            }
        }
    }

    fn dot(&mut self, x: f32, y: f32, scale: f32) {
        let s = Stroke { x0: x, y0: y, x1: x + 0.01, y1: y, width: 0.4, level: 1.0 };
        self.stroke(&s, scale, 0.0, 0.0, 0.25);
    }

    fn save(&self, path: &std::path::Path, zoom: usize) {
        let (w, h) = (self.w * zoom, self.h * zoom);
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let l = self.light[(y / zoom) * self.w + x / zoom];
                // Green phosphor; overexposed light goes white.
                let c = [l * 0.55 + (l - 1.0).max(0.0) * 0.6, l, l * 0.6 + (l - 1.0).max(0.0) * 0.5];
                for k in 0..3 {
                    rgb[(y * w + x) * 3 + k] = (c[k].clamp(0.0, 1.0) * 255.0) as u8;
                }
            }
        }
        let f = std::fs::File::create(path).unwrap();
        let mut e = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&rgb).unwrap();
    }
}

fn text(c: &mut Canvas, font: Font, s: &str, col: f32, row: f32, scale: f32) {
    let mut buf = Vec::new();
    for (i, ch) in s.chars().enumerate() {
        buf.clear();
        strokes_in(font, ch as u32, false, &mut buf);
        for st in &buf {
            c.stroke(st, scale, (col + i as f32) * CELL_W, row * CELL_H, 1.0);
        }
    }
}

fn repertoire() -> Vec<u32> {
    let ranges = [(0x20, 0x7E), (0xA0, 0xFF), (0x2013, 0x2014), (0x2018, 0x2019), (0x201C, 0x201D), (0x2022, 0x2022), (0x2026, 0x2026), (0x2212, 0x2212), (0x2500, 0x259F), (0xFFFD, 0xFFFD)];
    ranges.iter().flat_map(|&(a, b)| a..=b).collect()
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("out dir"));
    std::fs::create_dir_all(&out).unwrap();
    let codes = repertoire();
    for font in Font::ALL {
        // The specimen: 24 glyphs a row, each cell 6 pixels a unit with a
        // unit's margin, its code in small type under it.
        let (scale, per_row) = (6.0f32, 24usize);
        let (cw, ch) = ((CELL_W + 2.0) * scale, (CELL_H + 6.0) * scale);
        let rows = codes.len().div_ceil(per_row);
        let mut c = Canvas::new((cw * per_row as f32) as usize + 8, (ch * rows as f32) as usize + 8);
        let mut buf = Vec::new();
        for (i, &code) in codes.iter().enumerate() {
            let (gx, gy) = ((i % per_row) as f32 * (CELL_W + 2.0) + 1.0, (i / per_row) as f32 * (CELL_H + 6.0) + 1.0);
            // The cell's corners and the arcade grid's points, faint.
            for (x, y) in [(0.0, 0.0), (CELL_W, 0.0), (0.0, CELL_H), (CELL_W, CELL_H)] {
                c.dot(gx + x, gy + y, scale);
            }
            if i < per_row {
                for x in 0..=5 {
                    for y in 0..=13 {
                        c.dot(gx + 1.5 + x as f32, gy + 2.5 + y as f32, scale);
                    }
                }
            }
            buf.clear();
            strokes_in(font, code, false, &mut buf);
            for st in &buf {
                c.stroke(st, scale, gx, gy, 1.0);
            }
            // The code, a quarter the size.
            let label = format!("{code:04X}");
            let mut lb = Vec::new();
            for (k, d) in label.chars().enumerate() {
                lb.clear();
                strokes_in(Font::Arcade, d as u32, false, &mut lb);
                for st in &lb {
                    let s = Stroke { x0: st.x0 / 4.0, y0: st.y0 / 4.0, x1: st.x1 / 4.0, y1: st.y1 / 4.0, width: st.width / 2.0, ..*st };
                    c.stroke(&s, scale, gx + k as f32 * CELL_W / 4.0, gy + CELL_H + 0.5, 0.7);
                }
            }
        }
        c.save(&out.join(format!("specimen-{}.png", font.name())), 1);
        // Prose at 1×, 2×, 3×, each enlarged to 3×'s size.
        let cols = PROSE.iter().map(|l| l.chars().count()).max().unwrap() as f32;
        for n in [1usize, 2, 3] {
            let mut p = Canvas::new(((cols + 2.0) * CELL_W * n as f32) as usize, ((PROSE.len() as f32 + 1.0) * CELL_H * n as f32) as usize);
            for (r, l) in PROSE.iter().enumerate() {
                text(&mut p, font, l, 1.0, r as f32 + 0.5, n as f32);
            }
            p.save(&out.join(format!("prose-{}-{n}x.png", font.name())), 3 / n);
        }
    }
}
