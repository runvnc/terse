//! A machine's 3D scene put together over its glass (SPEC 12.3, 13.5): the
//! image of its screen with its Surfaces' sources blacked out, the panels
//! far to near (a Surface from a pyramid of that same image, before the
//! sources were blacked out; a Screen from the pyramid of another
//! machine's picture), its 3D lines over them. The software renderer
//! (`SoftScreens`) and terse-bus's compositor both put scenes together
//! with this; they differ only in what goes over the panels: the lines
//! drawn here from the projected segments, or a layer a 3D display driver
//! drew ([`Top`]).

use std::collections::HashMap;

use super::base::{IRect, Pixels, Pool, over, tile_rows};
use super::panel::{PanelMode, Placed, Pyramid, draw_panels};
use super::shapes::{Geom, Mode, Shape, visit};
use crate::display::{Eye, SceneLayers, eye_point, source_pixels};

/// What goes over the panels instead of the scene's own lines.
pub trait Top: Sync {
    /// Draws over the rows of `band` (an image covering `area`, the rows
    /// from `y0`), within `clip`.
    fn draw(&self, band: &mut [u32], area: IRect, y0: i32, clip: IRect);
}

/// A machine's 3D scene put together over its glass (SPEC 12.3, 13.5): its
/// screen's image, the panels over it, its 3D lines over them.
pub struct SceneImage {
    pub layers: SceneLayers,
    pub image: Pixels,
    /// The pyramid of the part of the screen's image its Surfaces show
    /// (if it has any), and that part.
    own: Option<(Pyramid, IRect)>,
    /// Whatever it was made from changed.
    pub stale: bool,
}

impl SceneImage {
    pub fn new(layers: SceneLayers) -> Self {
        SceneImage { layers, image: Pixels::default(), own: None, stale: true }
    }

    /// Takes the scene as it is now (none: the slot is emptied); true if it
    /// was there and went.
    pub fn update(slot: &mut Option<SceneImage>, layers: Option<SceneLayers>) -> bool {
        match (layers, slot.as_mut()) {
            (None, s) => s.is_some() && slot.take().is_some(),
            (Some(layers), Some(scene)) => {
                if scene.layers != layers {
                    scene.layers = layers;
                    scene.stale = true;
                }
                false
            }
            (Some(layers), None) => {
                *slot = Some(SceneImage::new(layers));
                false
            }
        }
    }

    /// Puts it together again if `base` changed (`damaged`) or anything
    /// else did: the pyramid its Surfaces show, then the image, with `top`
    /// over the panels if given, else the scene's lines. Returns whether
    /// it did.
    #[allow(clippy::too_many_arguments)]
    pub fn refresh(
        &mut self,
        base: &Pixels,
        damaged: bool,
        pyramids: &HashMap<u8, &Pyramid>,
        rect: (f32, f32, f32, f32),
        eye: Eye,
        mode: PanelMode,
        pool: &Pool,
        top: Option<&dyn Top>,
    ) -> bool {
        if !(self.stale || damaged || self.image.area != base.area) {
            return false;
        }
        self.stale = false;
        // The pyramid of the part of the image the sources cover (a pixel
        // more on each side, for the filtering at their edges).
        let mut region = IRect::EMPTY;
        for &r in &self.layers.rects {
            let (x0, y0, x1, y1) = source_pixels(rect, r);
            let b = IRect::new(x0 - 1, y0 - 1, x1 + 1, y1 + 1);
            region = if region.is_empty() { b } else { IRect::new(region.x0.min(b.x0), region.y0.min(b.y0), region.x1.max(b.x1), region.y1.max(b.y1)) };
        }
        let region = region.intersect(&base.area);
        if region.is_empty() {
            self.own = None;
        } else if damaged || self.own.as_ref().is_none_or(|o| o.1 != region) {
            let mut part = Pixels::new(region);
            for y in region.y0..region.y1 {
                let at = (y - region.y0) as usize * part.stride();
                part.data[at..at + region.width()].copy_from_slice(base.row(y, region.x0, region.x1));
            }
            self.own = Some((Pyramid::build(&part), region));
        }
        compose(self, base, pyramids, rect, eye, mode, pool, top);
        true
    }
}

/// Puts a machine's scene together into `scene.image`: the screen's image
/// `base` (drawn into `rect`) with its Surfaces' sources black, the panels
/// far to near (in perspective, or flat; a Surface from `scene.own`, the
/// others from `pyramids` by handle), the 3D lines (or `top`) over them,
/// a tile row per thread.
#[allow(clippy::too_many_arguments)]
fn compose(
    scene: &mut SceneImage,
    base: &Pixels,
    pyramids: &HashMap<u8, &Pyramid>,
    rect: (f32, f32, f32, f32),
    eye: Eye,
    mode: PanelMode,
    pool: &Pool,
    top: Option<&dyn Top>,
) {
    let SceneImage { layers, image, own, .. } = scene;
    if image.area != base.area {
        *image = Pixels::new(base.area);
    }
    image.data.copy_from_slice(&base.data);
    let area = image.area;
    let stride = area.width();
    // The sources are not drawn on the glass.
    for &r in &layers.rects {
        let (x0, y0, x1, y1) = source_pixels(rect, r);
        let b = IRect::new(x0, y0, x1, y1).intersect(&area);
        for y in b.y0..b.y1 {
            let row = (y - area.y0) as usize * stride;
            image.data[row + (b.x0 - area.x0) as usize..row + (b.x1 - area.x0) as usize].fill(0);
        }
    }
    let e = eye_point(eye);
    // A Surface's source, as a part of the screen's image.
    let s = rect.2 / terse_core::SCREEN_WIDTH as f32;
    let part = own.as_ref().map_or(area, |o| o.1);
    let (aw, ah) = (part.width().max(1) as f32, part.height().max(1) as f32);
    let window = |r: [u16; 4]| {
        let [x, y, w, h] = r.map(|v| v as f32);
        [(rect.0 + x * s - part.x0 as f32) / aw, (rect.1 + y * s - part.y0 as f32) / ah, w * s / aw, h * s / ah]
    };
    let placed: Vec<(Placed, Option<&Pyramid>)> = layers
        .panels
        .iter()
        .map(|p| {
            if p.is_surface() {
                (Placed::new(p, e, rect).with_window(window(p.source)), own.as_ref().map(|o| &o.0))
            } else {
                (Placed::new(p, e, rect), pyramids.get(&p.handle).copied())
            }
        })
        .collect();
    let geom = Geom::for_rect(rect);
    let shapes: Vec<Shape> = layers.lines.iter().map(|p| Shape::new(p, 0)).collect();
    let clip_all = IRect::covering(rect).intersect(&area);
    let bands = tile_rows(&mut image.data, stride, area);
    pool.each(bands, |(y0, band)| {
        let rows = (band.len() / stride.max(1)) as i32;
        let clip = clip_all.intersect(&IRect::new(area.x0, y0, area.x1, y0 + rows));
        if clip.is_empty() {
            return;
        }
        draw_panels(&placed, band, area, y0, clip, mode);
        if let Some(t) = top {
            t.draw(band, area, y0, clip);
            return;
        }
        for s in &shapes {
            let [r, g, b, a] = s.rgba();
            if a == 0 {
                continue;
            }
            let (c, alpha) = ([r as f32, g as f32, b as f32], a as f32 / 255.0);
            visit(s, &geom, Mode::Cover, clip, |x, y, v| {
                let p = &mut band[(y - y0) as usize * stride + (x - area.x0) as usize];
                *p = over(*p, c, alpha * v);
            });
        }
    });
}

