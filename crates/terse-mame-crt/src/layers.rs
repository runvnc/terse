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
use crate::pipeline::{Crt, Delivery, Gpu, Params};

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
    /// The last frame's beams and time, and for how many frames in a row
    /// they were the same.
    key: Vec<u8>,
    same: u32,
}

/// The internal resolution's ceiling by default: 960 lines (the 640 × 480
/// screen at twice its size). `TERSE_MAME_MAX_LINES` changes it.
pub const MAX_LINES: u32 = 960;

/// How a tube is drawn: what the host's shedding (`core.shed`) and the
/// options leave of the look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TubeLook {
    /// The phosphor's resolution is at most this many lines (the picture
    /// is sampled up from it).
    pub max_lines: u32,
    /// The bloom is drawn.
    pub bloom: bool,
    /// How the picture is read back.
    pub delivery: Delivery,
}

impl TubeLook {
    /// The look at shed level `level` (`core.shed`, DEVICES.md 7.2): 0 the
    /// whole look within `max_lines`; 1 at most 720 lines; 2 at most 480
    /// (the screen's own size); 3 that without the bloom.
    pub fn at(level: u8, max_lines: u32, delivery: Delivery) -> TubeLook {
        let lines = match level {
            0 => max_lines,
            1 => max_lines.min(720),
            _ => max_lines.min(480),
        };
        TubeLook { max_lines: lines, bloom: level < 3, delivery }
    }

    /// From the environment (`TERSE_MAME_MAX_LINES`, 480 to 4320;
    /// `TERSE_MAME_LATENCY=0`: wait for each picture) at level 0.
    pub fn from_env() -> TubeLook {
        let max_lines = std::env::var("TERSE_MAME_MAX_LINES").ok().and_then(|v| v.parse::<u32>().ok()).map_or(MAX_LINES, |v| v.clamp(480, 4320));
        let delivery = if std::env::var("TERSE_MAME_LATENCY").is_ok_and(|v| v == "0") { Delivery::Latest } else { Delivery::Pipelined };
        TubeLook { max_lines, bloom: true, delivery }
    }
}

/// The phosphor's size for a screen of `size` pixels: the same, or scaled
/// down (the aspect kept) to `max_lines`.
pub fn internal_size(size: (u32, u32), max_lines: u32) -> (u32, u32) {
    if size.1 <= max_lines {
        return size;
    }
    let w = (size.0 as u64 * max_lines as u64).div_ceil(size.1 as u64) as u32;
    (w.max(1), max_lines.max(1))
}

/// Frames of the same beams after which the phosphor is at its fixed point
/// (I = I × decay + beams) to within 2^-16 of its light: decay^n ≤ 2^-16.
/// Drawing it again would give the same picture.
pub fn steady_frames(persistence: f32, dt: f64) -> u32 {
    let k = dt / persistence.max(0.001) as f64;
    if k <= 0.0 || !k.is_finite() {
        return u32::MAX;
    }
    ((16.0 * std::f64::consts::LN_2 / k).ceil() as u32).saturating_add(1)
}

/// What drawing a frame cost, for `TERSE_MAME_TIMES`.
#[derive(Default)]
pub struct Times {
    /// Encoding and submitting the MAME pipeline, per frame drawn.
    pub frames: Vec<std::time::Duration>,
    /// Waiting for a picture read back.
    pub wait: Vec<std::time::Duration>,
    /// The picture compared with the layer and written where it changed.
    pub layer: Vec<std::time::Duration>,
    /// The whole PRESENT's work for a tube's layer (lines, pipeline,
    /// readback, layer).
    pub present: Vec<std::time::Duration>,
    pub beams: usize,
    /// Tubes made (a size, a resolution or the bloom changed).
    pub tubes: usize,
    /// Frames not drawn: the same beams, the phosphor at its fixed point.
    pub steady: usize,
}

/// Median, 95th percentile and maximum of `d` in milliseconds.
fn spread(d: &[std::time::Duration]) -> (f64, f64, f64) {
    let mut t: Vec<f64> = d.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    if t.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    t.sort_by(f64::total_cmp);
    let pct = |q: f64| t[((t.len() - 1) as f64 * q).round() as usize];
    (pct(0.5), pct(0.95), pct(1.0))
}

impl Times {
    pub fn report(&self, name: &str, adapter: &str) {
        if self.present.is_empty() {
            return;
        }
        let n = self.frames.len();
        let (m, p95, max) = spread(&self.frames);
        eprintln!(
            "{name}: {} PRESENTs, {n} frames drawn on {adapter} ({} steady, not drawn), {:.0} beams a frame drawn, {} tubes made",
            self.present.len(),
            self.steady,
            self.beams as f64 / n.max(1) as f64,
            self.tubes
        );
        eprintln!("{name}:   encoding and submitting: median {m:.2} ms, 95th {p95:.2} ms, max {max:.2} ms");
        for (what, d) in [("waiting for a picture", &self.wait), ("the picture into the layer", &self.layer), ("PRESENT in all", &self.present)] {
            let (m, p95, max) = spread(d);
            eprintln!("{name}:   {what}: median {m:.2} ms, 95th {p95:.2} ms, max {max:.2} ms");
        }
    }
}

/// Writes `src` (a row of the readback: BGRA bytes, a little-endian word
/// a pixel) over `dst`; true if anything changed.
fn copy_row(dst: &mut [u32], src: &[u8]) -> bool {
    #[cfg(target_endian = "little")]
    {
        // SAFETY: a u32 slice viewed as its bytes (alignment 1, the same
        // length in bytes); on a little-endian machine these bytes are the
        // words' as the readback holds them.
        let bytes = unsafe { std::slice::from_raw_parts_mut(dst.as_mut_ptr() as *mut u8, dst.len() * 4) };
        if bytes == src {
            return false;
        }
        bytes.copy_from_slice(src);
        true
    }
    #[cfg(not(target_endian = "little"))]
    {
        let mut changed = false;
        for (o, s) in dst.iter_mut().zip(src.chunks_exact(4)) {
            let w = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
            changed |= *o != w;
            *o = w;
        }
        changed
    }
}

/// The phosphor style with the MAME pipeline: the machine's beams this
/// frame, drawn into its tube (made for the screen's size, its phosphor at
/// most `look.max_lines` high), the picture written into the light layer at
/// the screen's place where it changed. `frames` is the number of frames
/// the machine has run; if none since the last picture, the phosphor is not
/// excited again (MAME's paused emulation). Frames whose beams are those of
/// the frames before, once the phosphor has reached its fixed point, are not
/// drawn again: the picture would be the same.
#[allow(clippy::too_many_arguments)]
pub fn draw_tube(
    drawn: &mut Option<Drawn>,
    tube: &mut Option<Tube>,
    gpu: &Gpu,
    params: Params,
    look: TubeLook,
    p: &Present,
    dir: &Path,
    instance: u32,
    slot: u8,
    id: u8,
    frames: u64,
    lines: impl FnOnce(&Screen) -> Vec<Line>,
    times: &mut Times,
) -> Result<LayerOut, String> {
    let began = std::time::Instant::now();
    let fresh = layer(drawn, p, dir, instance, slot, id)?;
    let d = drawn.as_mut().unwrap();
    let sp = screen_pixels(p);
    let size = (sp.width().max(1) as u32, sp.height().max(1) as u32);
    let internal = internal_size(size, look.max_lines);
    let mut first = false;
    if tube.as_ref().is_none_or(|t| t.crt.size() != size || t.crt.internal() != internal || t.crt.params != params || t.crt.bloom_on != look.bloom) {
        *tube = Some(Tube { crt: Crt::new(gpu, size, internal, params, look.bloom), frames, key: Vec::new(), same: 0 });
        first = true;
        times.tubes += 1;
    }
    let t = tube.as_mut().unwrap();
    let ran = frames.saturating_sub(t.frames);
    // MAME: the first frame assumes 1/60 s; no new emulated time, no new
    // excitation.
    let dt = if first { Some(1.0 / 60.0) } else { (ran > 0).then(|| ran as f64 / 60.0) };
    t.frames = frames;
    // The whole picture is needed now: a new layer, or one that held the
    // clean style.
    let whole = fresh || !d.light;
    let mut how = if dt.is_some() && !whole { look.delivery } else { Delivery::Flush };
    let mut submitted = false;
    if let Some(dt) = dt {
        // The lines in the phosphor's pixels: widths, σ and the bloom as
        // MAME has them at that resolution.
        let screen = Screen { size: internal };
        let instances = crate::beams::instances(&lines(&screen), params.beam_width);
        let mut key: Vec<u8> = instances.iter().flat_map(|b| b.bytes()).collect();
        key.extend(dt.to_le_bytes());
        if key == t.key {
            t.same = t.same.saturating_add(1);
        } else {
            t.same = 0;
            t.key = key;
        }
        if !first && t.same >= steady_frames(params.persistence, dt) {
            times.steady += 1;
            how = Delivery::Flush;
        } else {
            t.crt.frame(gpu, &instances, Some(dt));
            submitted = true;
            times.frames.push(t.crt.last.total);
            times.beams += t.crt.last.beams;
        }
    }
    if whole && !submitted {
        // Nothing new was drawn: the picture as it is, again.
        t.crt.frame(gpu, &[], Some(0.0));
    }
    let w = p.picture.0 as usize;
    let px = d.layer.pixels();
    if whole {
        px.fill(0);
    }
    let clip = sp.intersect(&IRect::of_size(p.picture));
    let (cx0, cx1) = ((clip.x0 - sp.x0) as usize, (clip.x1 - sp.x0) as usize);
    let mut rows: Option<(i32, i32)> = None;
    let handed = t.crt.deliver(gpu, how, |bytes, padded| {
        // Light (DEVICES.md 7.1): each word as read back, 0xFFRRGGBB, the
        // colour straight with full coverage (black: no light).
        for y in clip.y0..clip.y1 {
            let sy = (y - sp.y0) as usize;
            let src = &bytes[sy * padded + cx0 * 4..sy * padded + cx1 * 4];
            let out = &mut px[y as usize * w + clip.x0 as usize..][..cx1 - cx0];
            if copy_row(out, src) {
                rows = Some(rows.map_or((y, y + 1), |(a, _)| (a, y + 1)));
            }
        }
    });
    if handed {
        times.wait.push(t.crt.last.wait);
        times.layer.push(t.crt.last.copy);
    }
    let damage = if whole {
        vec![rect(IRect::of_size(p.picture))]
    } else {
        rows.map(|(y0, y1)| rect(IRect::new(clip.x0, y0, clip.x1, y1))).into_iter().collect()
    };
    times.present.push(began.elapsed());
    d.light = true;
    d.shapes.clear();
    d.bounds.clear();
    Ok(LayerOut { slot, id, new_file: fresh.then(|| d.layer.path.clone()), damage })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_resolution() {
        assert_eq!(internal_size((640, 480), 960), (640, 480));
        assert_eq!(internal_size((1280, 960), 960), (1280, 960));
        assert_eq!(internal_size((1920, 1440), 960), (1280, 960));
        assert_eq!(internal_size((2560, 1920), 720), (960, 720));
        assert_eq!(TubeLook::at(0, 960, Delivery::Pipelined).max_lines, 960);
        assert_eq!(TubeLook::at(1, 960, Delivery::Pipelined).max_lines, 720);
        assert_eq!(TubeLook::at(2, 960, Delivery::Pipelined), TubeLook { max_lines: 480, bloom: true, delivery: Delivery::Pipelined });
        assert_eq!(TubeLook::at(3, 960, Delivery::Pipelined), TubeLook { max_lines: 480, bloom: false, delivery: Delivery::Pipelined });
        assert_eq!(TubeLook::at(9, 600, Delivery::Latest).max_lines, 480);
    }

    #[test]
    fn steady_after_the_decay_is_spent() {
        // MAME's default 2 ms at 60 frames a second: e^(-8.3) a frame.
        assert_eq!(steady_frames(0.002, 1.0 / 60.0), 3);
        // The slider's longest, 100 ms: e^(-1/6) a frame.
        let n = steady_frames(0.1, 1.0 / 60.0);
        assert!((-(n as f64 - 1.0) / 6.0).exp() <= 2f64.powi(-16) && n < 80, "{n}");
        assert_eq!(steady_frames(0.002, 0.0), u32::MAX);
    }

    #[test]
    fn rows_compared_and_copied() {
        let mut dst = vec![0xFF00_0000u32; 4];
        let src: Vec<u8> = [0xFF00_0000u32, 0xFF01_0203, 0xFF00_0000, 0xFF00_0000].iter().flat_map(|w| w.to_le_bytes()).collect();
        assert!(copy_row(&mut dst, &src));
        assert_eq!(dst[1], 0xFF01_0203);
        assert!(!copy_row(&mut dst, &src));
    }
}
