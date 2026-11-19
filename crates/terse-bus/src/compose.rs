//! Composition (DEVICES.md 7): the drivers' layers, premultiplied pixels
//! in shared files, put together in z order over black where they changed,
//! into an XRGB8888 picture (softbuffer's and DRM's format).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::bus::LayerUpdate;
use crate::transport::Mapping;

/// The 640 × 480 screen inside a picture: the largest whole multiple of
/// the native size that fits, or a scaled-down fit, centred (as the
/// reference software renderer places a single machine's screen).
pub fn screen_rect(native: (u32, u32), picture: (u32, u32)) -> (f32, f32, f32, f32) {
    let (gw, gh) = native;
    let fit = (picture.0 as f32 / gw as f32).min(picture.1 as f32 / gh as f32);
    let scale = if fit >= 1.0 { fit.floor() } else { fit };
    let (w, h) = (gw as f32 * scale, gh as f32 * scale);
    (((picture.0 as f32 - w) / 2.0).floor(), ((picture.1 as f32 - h) / 2.0).floor(), w, h)
}

struct Layer {
    map: Option<Mapping>,
    path: Option<PathBuf>,
}

pub struct Compositor {
    pub size: (u32, u32),
    /// Layers in z order: (slot, id).
    order: Vec<(u8, u8)>,
    layers: HashMap<(u8, u8), Layer>,
    pub frame: Vec<u32>,
    /// Rectangles recomposed by the last `update`.
    pub damage: Vec<[i32; 4]>,
    pub pixels_composed: u64,
}

impl Compositor {
    /// `order`: the layers bottom to top.
    pub fn new(size: (u32, u32), order: Vec<(u8, u8)>) -> Self {
        let layers = order.iter().map(|&k| (k, Layer { map: None, path: None })).collect();
        Compositor { size, order, layers, frame: vec![0; (size.0 * size.1) as usize], damage: Vec::new(), pixels_composed: 0 }
    }

    /// Takes the drivers' news and composes where anything changed.
    pub fn update(&mut self, updates: &[LayerUpdate]) -> Result<(), String> {
        self.damage.clear();
        for u in updates {
            let Some(layer) = self.layers.get_mut(&(u.slot, u.id)) else { continue };
            if let Some(p) = &u.path {
                let map = Mapping::open(p, false)?;
                if map.len() < (self.size.0 * self.size.1 * 4) as usize {
                    return Err(format!("slot {}: layer file {} is too small", u.slot, p.display()));
                }
                layer.map = Some(map);
                layer.path = Some(p.clone());
            }
            for d in &u.damage {
                let r = [d[0].max(0), d[1].max(0), d[2].min(self.size.0 as i32), d[3].min(self.size.1 as i32)];
                if r[0] < r[2] && r[1] < r[3] {
                    self.damage.push(r);
                }
            }
        }
        let damage = merge(std::mem::take(&mut self.damage));
        let stack: Vec<&[u32]> = self
            .order
            .iter()
            .filter_map(|k| self.layers[k].map.as_ref())
            .map(|m| {
                // SAFETY: page-aligned mapping of at least width × height words.
                unsafe { std::slice::from_raw_parts(m.as_ptr() as *const u32, (self.size.0 * self.size.1) as usize) }
            })
            .collect();
        let w = self.size.0 as usize;
        for r in &damage {
            for y in r[1] as usize..r[3] as usize {
                let row = y * w;
                for x in r[0] as usize..r[2] as usize {
                    let i = row + x;
                    let (mut cr, mut cg, mut cb) = (0u32, 0u32, 0u32);
                    for l in &stack {
                        let p = l[i];
                        let a = p >> 24;
                        if a == 0 {
                            continue;
                        }
                        let (sr, sg, sb) = ((p >> 16) & 255, (p >> 8) & 255, p & 255);
                        if a == 255 {
                            (cr, cg, cb) = (sr, sg, sb);
                        } else {
                            let k = 255 - a;
                            cr = (sr + (cr * k + 127) / 255).min(255);
                            cg = (sg + (cg * k + 127) / 255).min(255);
                            cb = (sb + (cb * k + 127) / 255).min(255);
                        }
                    }
                    self.frame[i] = cr << 16 | cg << 8 | cb;
                }
            }
            self.pixels_composed += ((r[2] - r[0]) * (r[3] - r[1])) as u64;
        }
        self.damage = damage;
        Ok(())
    }

    /// RGBA bytes of the picture (for a PNG).
    pub fn rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.frame.len() * 4);
        for &p in &self.frame {
            out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, 255]);
        }
        out
    }
}

/// Damage rectangles merged into their bounding box when they overlap
/// much; few rectangles in practice.
fn merge(mut rects: Vec<[i32; 4]>) -> Vec<[i32; 4]> {
    let area = |r: &[i32; 4]| (r[2] - r[0]) as i64 * (r[3] - r[1]) as i64;
    let mut changed = true;
    while changed {
        changed = false;
        'outer: for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                let (a, b) = (rects[i], rects[j]);
                let u = [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
                let overlap = a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3];
                if overlap || area(&u) <= area(&a) + area(&b) {
                    rects[i] = u;
                    rects.swap_remove(j);
                    changed = true;
                    break 'outer;
                }
            }
        }
    }
    rects
}

/// Writes RGBA pixels to a PNG file.
pub fn write_png(path: &std::path::Path, rgba: &[u8], (w, h): (u32, u32)) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())
}
