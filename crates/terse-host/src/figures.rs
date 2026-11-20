//! Figures on the page (SPEC §page.document): paths rasterised with tiny-skia at the
//! target's pixel size, antialiased, on a worker thread. The page layer asks
//! for the figures near the view and keeps only those (see `page.rs`), so
//! the textures stay bounded whatever the document holds.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use terse_core::page::{DEFAULT_STROKE_WIDTH, FigurePath, LayoutKey, Segment};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

/// A figure rasterised for one look: the layout it belongs to, its index
/// there, the pixel size and whether it is drawn as outlines.
pub type FigureKey = (LayoutKey, usize, (u32, u32), bool);

/// Rasterises `paths` into a `size` pixel image, premultiplied RGBA, at
/// `scale` pixels per screen unit (coordinates are sixteenths). Paths are
/// filled (non-zero) then stroked with round joins and caps. With
/// `outline` (the phosphor style, fill mode 1) nothing is filled: a path
/// with a fill but no stroke is stroked one unit wide in its fill colour.
pub fn rasterize(paths: &[FigurePath], size: (u32, u32), scale: f32, outline: bool) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(size.0.max(1), size.1.max(1))?;
    let k = scale / 16.0;
    let t = Transform::from_scale(k, k);
    let paint = |[r, g, b, a]: [u8; 4]| {
        let mut p = Paint::default();
        p.set_color_rgba8(r, g, b, a);
        p.anti_alias = true;
        p
    };
    for p in paths {
        let Some(path) = build(&p.segments) else { continue };
        if let Some(c) = p.fill.filter(|_| !outline) {
            pixmap.fill_path(&path, &paint(c), FillRule::Winding, t, None);
        }
        let stroke = match (p.stroke, p.fill) {
            (Some(s), _) => Some(s),
            (None, Some(c)) if outline => Some((c, DEFAULT_STROKE_WIDTH)),
            _ => None,
        };
        if let Some((c, w)) = stroke {
            let stroke = Stroke { width: w as f32, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
            pixmap.stroke_path(&path, &paint(c), &stroke, t, None);
        }
    }
    Some(pixmap)
}

fn build(segments: &[Segment]) -> Option<tiny_skia::Path> {
    let mut b = PathBuilder::with_capacity(segments.len(), segments.len() * 3);
    let f = |v: i16| v as f32;
    for s in segments {
        match *s {
            Segment::Move(x, y) => b.move_to(f(x), f(y)),
            Segment::Line(x, y) => b.line_to(f(x), f(y)),
            Segment::Quad(x1, y1, x, y) => b.quad_to(f(x1), f(y1), f(x), f(y)),
            Segment::Cubic(x1, y1, x2, y2, x, y) => b.cubic_to(f(x1), f(y1), f(x2), f(y2), f(x), f(y)),
            Segment::Close => b.close(),
        }
    }
    b.finish()
}

/// A request for the worker.
pub struct Job {
    pub key: FigureKey,
    pub paths: Arc<Vec<FigurePath>>,
    pub scale: f32,
}

/// A finished figure: premultiplied RGBA of the key's size.
pub struct Done {
    pub key: FigureKey,
    pub data: Vec<u8>,
    pub ms: f64,
}

/// Runs `job` (on any thread).
pub fn run(job: &Job) -> Done {
    let start = std::time::Instant::now();
    let (w, h) = job.key.2;
    let data = rasterize(&job.paths, (w, h), job.scale, job.key.3)
        .map(Pixmap::take)
        .unwrap_or_else(|| vec![0; (w.max(1) * h.max(1) * 4) as usize]);
    Done { key: job.key, data, ms: start.elapsed().as_secs_f64() * 1000.0 }
}

/// The rasterising thread. Requests that are no longer wanted when their
/// turn comes (the page scrolled on, the window was resized) are skipped.
pub struct Rasterizer {
    jobs: Sender<Job>,
    pub done: Receiver<Done>,
    pub wanted: Arc<Mutex<HashSet<FigureKey>>>,
}

impl Rasterizer {
    pub fn start() -> Self {
        let (jobs, rx) = std::sync::mpsc::channel::<Job>();
        let (tx, done) = std::sync::mpsc::channel();
        let wanted: Arc<Mutex<HashSet<FigureKey>>> = Arc::default();
        let still = wanted.clone();
        std::thread::Builder::new()
            .name("page figures".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    if !still.lock().unwrap().contains(&job.key) {
                        continue;
                    }
                    if tx.send(run(&job)).is_err() {
                        return;
                    }
                }
            })
            .expect("start the figure thread");
        Rasterizer { jobs, done, wanted }
    }

    pub fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terse_core::page::{Figure, path_op};

    fn pt(op: u8, xy: &[i16]) -> Vec<u8> {
        let mut v = vec![op];
        for c in xy {
            v.extend_from_slice(&c.to_le_bytes());
        }
        v
    }

    fn figure(w: u16, h: u16, path: Vec<u8>) -> Vec<FigurePath> {
        Figure { width: w, height: h, rgb: [200, 200, 200], anchor: 0, path }.paths()
    }

    fn pixel(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let c = p.pixel(x, y).unwrap();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }

    #[test]
    fn fills_strokes_and_outlines() {
        // A red square (10..30 units) with the default stroke (text colour,
        // one unit), at 2 pixels per unit.
        let mut path = vec![path_op::FILL, 255, 0, 0, 255];
        path.extend(pt(path_op::MOVE, &[160, 160]));
        path.extend(pt(path_op::LINE, &[480, 160]));
        path.extend(pt(path_op::LINE, &[480, 480]));
        path.extend(pt(path_op::LINE, &[160, 480]));
        path.push(path_op::CLOSE);
        let paths = figure(40, 40, path);
        let p = rasterize(&paths, (80, 80), 2.0, false).unwrap();
        assert_eq!(pixel(&p, 40, 40), [255, 0, 0, 255], "inside: filled");
        assert_eq!(pixel(&p, 20, 40), [200, 200, 200, 255], "on the edge: stroked");
        assert_eq!(pixel(&p, 5, 5), [0, 0, 0, 0], "outside: clear");
        let o = rasterize(&paths, (80, 80), 2.0, true).unwrap();
        assert_eq!(pixel(&o, 40, 40), [0, 0, 0, 0], "outlines: no fill");
        assert_eq!(pixel(&o, 20, 40), [200, 200, 200, 255]);
        // A fill without a stroke is outlined in its colour.
        let mut path = vec![path_op::STROKE, 0, 0, 0, 0, 0, 0, path_op::FILL, 0, 255, 0, 255];
        path.extend(pt(path_op::MOVE, &[160, 160]));
        path.extend(pt(path_op::LINE, &[480, 160]));
        path.extend(pt(path_op::LINE, &[480, 480]));
        path.push(path_op::CLOSE);
        let o = rasterize(&figure(40, 40, path), (80, 80), 2.0, true).unwrap();
        assert_eq!(pixel(&o, 40, 20), [0, 255, 0, 255]);
        assert_eq!(pixel(&o, 50, 30), [0, 0, 0, 0]);
    }

    /// A Figure command.
    pub fn figure_cmd(w: u16, h: u16, path: &[u8]) -> Vec<u8> {
        let mut v = vec![terse_core::page::op::FIGURE];
        v.extend_from_slice(&w.to_le_bytes());
        v.extend_from_slice(&h.to_le_bytes());
        v.push(1);
        v.extend_from_slice(&(path.len() as u16).to_le_bytes());
        v.extend_from_slice(path);
        v
    }

    /// Hostile documents of at most 64 KiB, each as many copies of one
    /// figure as fit: (name, document). Each figure is as costly as the
    /// caps allow in some way.
    pub fn hostile_documents() -> Vec<(&'static str, Vec<u8>)> {
        let fill = |p: &mut Vec<u8>, i: usize| p.extend_from_slice(&[path_op::FILL, (i * 40) as u8, 200, 100, 255]);
        let no_stroke = [path_op::STROKE, 0, 0, 0, 0, 0, 0];
        let stroke = |p: &mut Vec<u8>, w: u16| {
            p.extend_from_slice(&[path_op::STROKE, 255, 255, 255, 255]);
            p.extend_from_slice(&w.to_le_bytes());
        };
        let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
        // Full-height zigzag cubics in the box, filled: the length cap.
        let mut p = no_stroke.to_vec();
        fill(&mut p, 0);
        p.extend(pt(path_op::MOVE, &[0, 0]));
        for i in 0..4000 {
            let x = ((i * 97) % 1024) as i16 * 16;
            let y = if i % 2 == 0 { 16383 } else { 0 };
            p.extend(pt(path_op::CUBIC, &[16383 - x, y, x, 16383 - y, x, y]));
            if i % 4 == 3 {
                fill(&mut p, i);
                p.extend(pt(path_op::MOVE, &[x, y]));
            }
        }
        cases.push(("full-height cubics, filled", p));
        // The same, stroked 1 unit and filled.
        let mut p = Vec::new();
        stroke(&mut p, 16);
        fill(&mut p, 0);
        p.extend(pt(path_op::MOVE, &[0, 0]));
        for i in 0..4000 {
            let x = ((i * 97) % 1024) as i16 * 16;
            let y = if i % 2 == 0 { 16383 } else { 0 };
            p.extend(pt(path_op::CUBIC, &[16383 - x, y, x, 16383 - y, x, y]));
            if i % 4 == 3 {
                fill(&mut p, i);
                p.extend(pt(path_op::MOVE, &[x, y]));
            }
        }
        cases.push(("full-height cubics, filled and stroked", p));
        // Many tiny cubics, stroked: the most segments.
        let mut p = Vec::new();
        stroke(&mut p, 16);
        p.extend(pt(path_op::MOVE, &[0, 0]));
        for i in 0..5000 {
            let (x, y) = (((i * 53) % 1000) as i16 * 16, ((i * 31) % 1000) as i16 * 16);
            p.extend(pt(path_op::CUBIC, &[x + 16, y, x, y + 16, x + 8, y + 8]));
            if i % 64 == 63 {
                stroke(&mut p, 16);
            }
        }
        cases.push(("5,000 tiny cubics, stroked", p));
        // Many small paths: a fill change before each triangle.
        let mut p = no_stroke.to_vec();
        for i in 0..3200 {
            fill(&mut p, i);
            let (x, y) = (((i * 53) % 1000) as i16 * 16, ((i * 31) % 1000) as i16 * 16);
            p.extend(pt(path_op::MOVE, &[x, y]));
            p.extend(pt(path_op::LINE, &[x + 300, y + 20]));
            p.extend(pt(path_op::LINE, &[x + 40, y + 300]));
        }
        cases.push(("3,200 small filled paths", p));
        // Strokes as wide as allowed (4,096 units): round joins over everything.
        let mut p = Vec::new();
        stroke(&mut p, 65535);
        p.extend(pt(path_op::MOVE, &[0, 0]));
        for i in 0..100 {
            p.extend(pt(path_op::LINE, &[((i * 97) % 1024) as i16 * 16, if i % 2 == 0 { 16383 } else { 0 }]));
            stroke(&mut p, 65535);
            p.extend(pt(path_op::MOVE, &[((i * 97) % 1024) as i16 * 16, if i % 2 == 0 { 16383 } else { 0 }]));
        }
        cases.push(("strokes 4,096 units wide", p));
        let mut cases: Vec<(&str, (u16, u16), Vec<u8>)> = cases.into_iter().map(|(n, p)| (n, (1024, 1024), p)).collect();
        // Many smaller figures, each up to its caps.
        let mut p = no_stroke.to_vec();
        p.extend(pt(path_op::MOVE, &[0, 0]));
        for i in 0..220 {
            let x = ((i * 97) % 640) as i16 * 16;
            let y = if i % 2 == 0 { 4095 } else { 0 };
            p.extend(pt(path_op::CUBIC, &[10239 - x, y, x, 4095 - y, x, y]));
            if i % 30 == 0 {
                fill(&mut p, i);
                p.extend(pt(path_op::MOVE, &[x, y]));
            }
        }
        cases.push(("figures 640 x 256, cubics up to the caps", (640, 256), p));
        let mut p = no_stroke.to_vec();
        for i in 0..512 {
            fill(&mut p, i);
            let (x, y) = (((i * 53) % 600) as i16 * 16, ((i * 31) % 100) as i16 * 16);
            p.extend(pt(path_op::MOVE, &[x, y]));
            p.extend(pt(path_op::LINE, &[x + 600, y + 20]));
            p.extend(pt(path_op::LINE, &[x + 40, y + 450]));
        }
        cases.push(("figures 640 x 128, 512 paths each", (640, 128), p));
        // Narrow strips of full-height zigzag lines, a path each: long
        // paths with small boxes, up to the length cap.
        let mut p = Vec::new();
        stroke(&mut p, 16);
        for i in 0..400i16 {
            fill(&mut p, i as usize);
            let x = (i * 40) % 16000;
            p.extend(pt(path_op::MOVE, &[x, 0]));
            p.extend(pt(path_op::LINE, &[x + 16, 16383]));
            p.extend(pt(path_op::LINE, &[x + 32, 0]));
        }
        cases.push(("full-height strips, a path each", (1024, 1024), p));
        let mut out = Vec::new();
        for (name, (w, h), path) in cases {
            let path = &path[..path.len().min(65_536 - 16)];
            let one = figure_cmd(w, h, path);
            let mut doc = Vec::new();
            while doc.len() + one.len() <= 65_536 {
                doc.extend_from_slice(&one);
            }
            out.push((name, doc));
        }
        out
    }

    /// A hostile document's figure: its paths and the size shown in a
    /// rectangle 640 units wide.
    fn hostile_figure(doc: &[u8]) -> (Vec<FigurePath>, (u16, u16)) {
        let f = terse_core::page::parse(doc).into_iter().find_map(|p| p.figure).unwrap();
        (f.paths(), (f.width.min(640), f.height))
    }

    /// The hostile documents: their worst figure alone at several scales,
    /// then the whole document laid out and the figures of the first
    /// screen (and the margin below it) rasterised, as the host does:
    /// `cargo test --release -p terse-host figure_raster_time -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn figure_raster_time() {
        let mut typesetter = crate::page::Typesetter::new(&crate::page::PageFonts::default()).ok();
        for (name, doc) in hostile_documents() {
            let (paths, size) = hostile_figure(&doc);
            for scale in [1.0f32, 2.0, 3.0] {
                let px = ((size.0 as f32 * scale).ceil() as u32, (size.1 as f32 * scale).ceil() as u32);
                let start = std::time::Instant::now();
                let _ = rasterize(&paths, px, scale, false);
                let ms = start.elapsed().as_secs_f64() * 1000.0;
                let start = std::time::Instant::now();
                let _ = rasterize(&paths, px, scale, true);
                let outline_ms = start.elapsed().as_secs_f64() * 1000.0;
                println!("{name}: one figure, {} paths drawn, scale {scale}: {ms:.1} ms (outlines {outline_ms:.1} ms)", paths.len());
            }
            let Some(t) = typesetter.as_mut() else { continue };
            let start = std::time::Instant::now();
            let layout = t.layout((1, 512), &doc, 512);
            let layout_ms = start.elapsed().as_secs_f64() * 1000.0;
            let start = std::time::Instant::now();
            let mut n = 0;
            for f in layout.figures.iter().take_while(|f| f.top < 480.0 * 1.5) {
                let px = ((f.width * 2.0).ceil() as u32, (f.height * 2.0).ceil() as u32);
                let _ = rasterize(&f.paths, px, 2.0, false);
                n += 1;
            }
            let raster_ms = start.elapsed().as_secs_f64() * 1000.0;
            println!(
                "{name}: {} bytes, {} figures: layout {layout_ms:.1} ms; first screen and margin ({n} figures) at scale 2: {raster_ms:.1} ms",
                doc.len(),
                layout.figures.len()
            );
        }
    }
}
