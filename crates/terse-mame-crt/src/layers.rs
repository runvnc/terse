//! Layers (DEVICES.md 7): the clean style drawn as the reference drivers
//! draw it (terse-host's shapes, premultiplied), and a light layer filled
//! with the MAME pipeline's picture (DEVICES.md 7.1, with `vector.tube`).

use std::path::Path;

use terse_bus::driver::{LayerFile, LayerOut, Present};
use terse_core::Primitive;
use terse_devices::{clear, over4, rect, scissor, union};
use terse_host::soft::base::IRect;
use terse_host::soft::shapes::{Geom, Mode, Shape, visit};

use crate::beams::{Line, Screen};
use crate::pipeline::{Crt, Gpu, Params};

/// A layer and what was drawn in it.
pub struct Drawn {
    pub picture: (u32, u32),
    pub screen: (f32, f32, f32, f32),
    pub layer: LayerFile,
    shapes: Vec<Shape>,
    bounds: Vec<IRect>,
    /// Drawn as light last time.
    light: bool,
}

/// The layer for a picture, made again when the picture or the screen in
/// it changed; true if new (its first damage is the whole picture).
pub fn layer(drawn: &mut Option<Drawn>, p: &Present, dir: &Path, instance: u32, slot: u8, id: u8) -> Result<bool, String> {
    if drawn.as_ref().is_none_or(|d| d.picture != p.picture || d.screen != p.screen) {
        let layer = LayerFile::create(dir, instance, slot, id, p.picture)?;
        *drawn = Some(Drawn { picture: p.picture, screen: p.screen, layer, shapes: Vec::new(), bounds: Vec::new(), light: false });
        return Ok(true);
    }
    Ok(false)
}

/// The clean style: `shapes` over transparent, premultiplied, where they
/// or the last ones were (as terse-dev-vector draws its layer).
pub fn draw_clean(drawn: &mut Option<Drawn>, p: &Present, dir: &Path, instance: u32, slot: u8, id: u8, prims: &[Primitive]) -> Result<LayerOut, String> {
    let fresh = layer(drawn, p, dir, instance, slot, id)?;
    let d = drawn.as_mut().unwrap();
    let shapes: Vec<Shape> = prims.iter().map(|s| Shape::new(s, 0)).collect();
    if !fresh && !d.light && shapes == d.shapes {
        return Ok(LayerOut { slot, id, new_file: None, damage: Vec::new() });
    }
    let geom = Geom::for_rect(p.screen);
    let bounds: Vec<IRect> = shapes.iter().map(|s| s.bounds(&geom, false)).collect();
    let clip_all = scissor(p.screen, p.picture);
    let mut damage = if fresh || d.light { IRect::of_size(p.picture) } else { IRect::EMPTY };
    for b in d.bounds.iter().chain(&bounds) {
        damage = union(damage, *b);
    }
    let damage = damage.intersect(&IRect::of_size(p.picture));
    let w = p.picture.0 as usize;
    let px = d.layer.pixels();
    if !damage.is_empty() {
        clear(px, w, damage);
        let clip = damage.intersect(&clip_all);
        for (s, b) in shapes.iter().zip(&bounds) {
            if b.intersect(&clip).is_empty() {
                continue;
            }
            let [r, g, bl, a] = s.rgba();
            if a == 0 {
                continue;
            }
            let c = [r as f32, g as f32, bl as f32];
            let alpha = a as f32 / 255.0;
            visit(s, &geom, Mode::Cover, clip, |x, y, v| {
                let q = &mut px[y as usize * w + x as usize];
                *q = over4(*q, c, alpha * v);
            });
        }
    }
    d.shapes = shapes;
    d.bounds = bounds;
    d.light = false;
    Ok(LayerOut { slot, id, new_file: fresh.then(|| d.layer.path.clone()), damage: if damage.is_empty() { vec![] } else { vec![rect(damage)] } })
}

/// A layer left empty (transparent).
pub fn draw_empty(drawn: &mut Option<Drawn>, p: &Present, dir: &Path, instance: u32, slot: u8, id: u8) -> Result<LayerOut, String> {
    let fresh = layer(drawn, p, dir, instance, slot, id)?;
    let d = drawn.as_mut().unwrap();
    let had = fresh || !d.bounds.is_empty() || d.light;
    if had {
        d.layer.pixels().fill(0);
    }
    d.shapes.clear();
    d.bounds.clear();
    d.light = false;
    Ok(LayerOut { slot, id, new_file: fresh.then(|| d.layer.path.clone()), damage: if had { vec![[0, 0, p.picture.0 as i32, p.picture.1 as i32]] } else { vec![] } })
}

/// The screen's rectangle in whole pixels (the phosphor style's PRESENT
/// puts its top left on whole pixels, DEVICES.md 7.1).
pub fn screen_pixels(p: &Present) -> IRect {
    let (x, y, w, h) = p.screen;
    let (x0, y0) = (x as i32, y as i32);
    IRect::new(x0, y0, x0 + w.round() as i32, y0 + h.round() as i32)
}

/// One machine's phosphor and the emulated time it has seen.
pub struct Tube {
    pub crt: Crt,
    /// The machine's frames (ENDs) when it was last drawn.
    pub frames: u64,
}

/// What drawing a frame cost, for `TERSE_MAME_TIMES`.
#[derive(Default)]
pub struct Times {
    pub frames: Vec<std::time::Duration>,
    pub beams: usize,
}

impl Times {
    pub fn report(&self, name: &str, adapter: &str) {
        if self.frames.is_empty() {
            return;
        }
        let mut t: Vec<f64> = self.frames.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
        t.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mean = t.iter().sum::<f64>() / t.len() as f64;
        let pct = |q: f64| t[((t.len() - 1) as f64 * q).round() as usize];
        eprintln!(
            "{name}: {} frames on {adapter}: mean {mean:.2} ms, median {:.2} ms, 95th {:.2} ms, max {:.2} ms (MAME pipeline and readback), {:.0} beams a frame",
            t.len(),
            pct(0.5),
            pct(0.95),
            pct(1.0),
            self.beams as f64 / t.len() as f64
        );
    }
}

/// The phosphor style with the MAME pipeline: the machine's beams this
/// frame, drawn into its tube (made for the screen's size), the picture
/// written into the light layer at the screen's place. `frames` is the
/// number of frames the machine has run; if none since the last picture,
/// the phosphor is not excited again (MAME's paused emulation) and the
/// layer keeps its picture.
#[allow(clippy::too_many_arguments)]
pub fn draw_tube(
    drawn: &mut Option<Drawn>,
    tube: &mut Option<Tube>,
    gpu: &Gpu,
    params: Params,
    p: &Present,
    dir: &Path,
    instance: u32,
    slot: u8,
    id: u8,
    frames: u64,
    lines: impl FnOnce(&Screen) -> Vec<Line>,
    times: &mut Times,
) -> Result<LayerOut, String> {
    let fresh = layer(drawn, p, dir, instance, slot, id)?;
    let d = drawn.as_mut().unwrap();
    let sp = screen_pixels(p);
    let size = (sp.width().max(1) as u32, sp.height().max(1) as u32);
    let mut first = false;
    if tube.as_ref().is_none_or(|t| t.crt.size() != size || t.crt.params != params) {
        *tube = Some(Tube { crt: Crt::new(gpu, size.0, size.1, params), frames });
        first = true;
    }
    let t = tube.as_mut().unwrap();
    let ran = frames.saturating_sub(t.frames);
    // MAME: the first frame assumes 1/60 s; no new emulated time, no new
    // excitation.
    let dt = if first { Some(1.0 / 60.0) } else { (ran > 0).then(|| ran as f64 / 60.0) };
    if dt.is_none() && !fresh && d.light {
        return Ok(LayerOut { slot, id, new_file: None, damage: Vec::new() });
    }
    t.frames = frames;
    let screen = Screen { size };
    let lines = if dt.is_some() { lines(&screen) } else { Vec::new() };
    let instances = crate::beams::instances(&lines, params.beam_width);
    t.crt.frame(gpu, &instances, dt);
    if dt.is_some() {
        times.frames.push(t.crt.last.total);
        times.beams += t.crt.last.beams;
    }
    let rgba = t.crt.pixels();
    let w = p.picture.0 as usize;
    let px = d.layer.pixels();
    if fresh || !d.light {
        px.fill(0);
    }
    let clip = sp.intersect(&IRect::of_size(p.picture));
    for y in clip.y0..clip.y1 {
        let sy = (y - sp.y0) as usize;
        let row = &rgba[sy * size.0 as usize * 4..][..size.0 as usize * 4];
        let out = &mut px[y as usize * w..][..w];
        for x in clip.x0..clip.x1 {
            let s = &row[(x - sp.x0) as usize * 4..][..4];
            // Light (DEVICES.md 7.1): the colour straight in bits 0–23,
            // full coverage; no light, transparent.
            out[x as usize] = if s[0] | s[1] | s[2] == 0 { 0 } else { 0xFF00_0000 | (s[0] as u32) << 16 | (s[1] as u32) << 8 | s[2] as u32 };
        }
    }
    let damage = if fresh || !d.light { IRect::of_size(p.picture) } else { clip };
    d.light = true;
    d.shapes.clear();
    d.bounds.clear();
    Ok(LayerOut { slot, id, new_file: fresh.then(|| d.layer.path.clone()), damage: vec![rect(damage)] })
}
