//! The core's compositor (DEVICES.md 7): the picture of a system (SPEC
//! §machines.screens) or of a single machine, put together from the drivers' layers
//! as terse-host's software renderer puts its screens together: each
//! machine's layers composed into its screen's image at the size it is
//! shown at (the drivers draw each machine at that size), the shell's
//! image at the bottom, the children's pictures over it in handle order, a
//! scene's panels (Screens of other machines, a machine's own Surfaces)
//! and its 3D layer over them, and the host's consent prompt over
//! everything. The scene, the panels and their pyramids are terse-host's
//! (`soft::scene`, `soft::panel`); the frame is XRGB8888 (softbuffer's and
//! DRM's format).
//!
//! A machine in the phosphor style (SPEC §vector.phosphor) is drawn as
//! terse-host draws it, with terse-host's tube (`soft::screen`,
//! `soft::phosphor`): its drivers draw their layers as light (DEVICES.md
//! 7.1, `core.light`: no backgrounds, straight colour), which is the text's
//! light, and the core draws the beam of the vector displays' lists itself
//! (`vector.list`, `vector3.list`), with the afterglow and the halo.
//!
//! The machines a shell shows share the system ink (SPEC §machines.ink):
//! terse-core counts it from the pictures and the lists (`vector.list`
//! for the 2D lists, which live in the vector driver); a picture that does
//! not fit is a plate, drawn here without asking its drivers, and each
//! machine's lists are drawn within the caps it is given (`core.caps`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use terse_core::devices::Devices;
use terse_core::system_ink::Shares;
use terse_core::{COLS, Machine, System, Terminal, Vector, Vector3};
use terse_host::display::{
    Caps, Eye, GlyphSource, SceneLayers, VectorScene, cell_width_for, grid_rect, panel_cell_width, panel_width_px, picture_rect, plate_panels,
    scene_layers, surface_layers, system_rect,
};
use terse_host::phosphor::Look;
use terse_host::soft::base::{Damage, IRect, Pixels, Pool, tile_rows};
use terse_host::soft::panel::{PanelMode, Pyramid};
use terse_host::soft::phosphor::LightLayers;
use terse_host::soft::scene::{SceneImage, Top};
use terse_host::soft::screen::SoftScreen;
use terse_net::Prompt;

use crate::bus::{BusDevices, LayerUpdate};
use crate::proto::property;
use crate::transport::Mapping;

/// The 640 × 480 screen inside a picture: the largest whole multiple of
/// the native size that fits, or a scaled-down fit, centred (as the
/// reference software renderer places a single machine's screen).
pub fn screen_rect(native: (u32, u32), picture: (u32, u32)) -> (f32, f32, f32, f32) {
    grid_rect(native, picture)
}

/// `a` premultiplied over `b` (XRGB).
#[inline]
fn over(p: u32, dst: u32) -> u32 {
    let a = p >> 24;
    if a == 255 {
        return p & 0xFF_FFFF;
    }
    let k = 255 - a;
    let ch = |s: u32, d: u32| (s + (d * k + 127) / 255).min(255);
    ch((p >> 16) & 255, (dst >> 16) & 255) << 16 | ch((p >> 8) & 255, (dst >> 8) & 255) << 8 | ch(p & 255, dst & 255)
}

/// A layer file mapped read-only.
struct Layer {
    map: Mapping,
    path: PathBuf,
    size: (u32, u32),
}

impl Layer {
    fn pixels(&self) -> &[u32] {
        // SAFETY: page-aligned mapping of at least width × height words.
        unsafe { std::slice::from_raw_parts(self.map.as_ptr() as *const u32, (self.size.0 * self.size.1) as usize) }
    }
}

/// How a machine is shown in a draw, as the compositor decided it.
#[derive(Clone)]
pub struct Show {
    /// Its picture is a plate (SPEC §machines.ink): a plain rectangle.
    pub plate: bool,
    /// In the phosphor style (the program's, or forced): drawn with the tube.
    pub phosphor: bool,
    /// Device 3's state (`vector.list`), for the beam.
    pub vector: Option<Vector>,
    /// Device 9's list, drawn in the tube (not over a scene's panels).
    pub list3: Option<Vec<u8>>,
    /// The ink caps its lists are drawn within.
    pub caps: Caps,
    /// Its Surfaces' sources, which give no light.
    pub excluded: Vec<[u16; 4]>,
    /// The afterglow: `Some(true)` moves on (it ran a frame), `Some(false)`
    /// the beam is drawn again, `None` the tube is drawn only if it must be.
    pub advance: Option<bool>,
    /// The slots whose layers are light in the phosphor style.
    pub light_slots: Vec<u8>,
    /// In the phosphor style, the vector driver draws the tube itself
    /// (`vector.tube`, DEVICES.md 7.2): its layers are light too, and the
    /// core adds the layers' light as it is, without a tube of its own.
    pub own_tube: bool,
}

impl Default for Show {
    fn default() -> Self {
        Show {
            plate: false,
            phosphor: false,
            vector: None,
            list3: None,
            caps: Caps::FULL,
            excluded: Vec::new(),
            advance: None,
            light_slots: Vec::new(),
            own_tube: false,
        }
    }
}

/// Linear light of each 8-bit sRGB value.
fn linear_table() -> &'static [f32; 256] {
    static T: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    T.get_or_init(|| std::array::from_fn(|i| terse_host::soft::phosphor::to_linear(i as f32 / 255.0)))
}

/// The 8-bit sRGB value of linear light (clipped to 0–1), computed.
fn srgb8_exact(c: f32) -> u32 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u32
}

/// Buckets of [`srgb8`]'s table over 0–1.
const SRGB_BUCKETS: usize = 4096;

/// [`srgb8_exact`]'s steps: the least linear value giving each 8-bit value
/// (found by bisection on the function itself, so the table agrees with
/// it exactly), and per bucket of the linear range the value at its start.
fn srgb8_table() -> &'static ([f32; 257], [u8; SRGB_BUCKETS]) {
    static T: std::sync::OnceLock<([f32; 257], [u8; SRGB_BUCKETS])> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut steps = [f32::INFINITY; 257];
        steps[0] = f32::NEG_INFINITY;
        for (k, step) in steps.iter_mut().enumerate().take(256).skip(1) {
            // Positive floats are ordered as their bits.
            let (mut lo, mut hi) = (0u32, 1.0f32.to_bits());
            if srgb8_exact(1.0) < k as u32 {
                continue;
            }
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if srgb8_exact(f32::from_bits(mid)) >= k as u32 { hi = mid } else { lo = mid + 1 }
            }
            *step = f32::from_bits(lo);
        }
        let mut start = [0u8; SRGB_BUCKETS];
        for (i, s) in start.iter_mut().enumerate() {
            *s = srgb8_exact(i as f32 / SRGB_BUCKETS as f32) as u8;
        }
        (steps, start)
    })
}

/// The 8-bit sRGB value of linear light (clipped to 0–1): [`srgb8_exact`]
/// by table (no power function per pixel).
#[inline]
fn srgb8(c: f32) -> u32 {
    let (steps, start) = srgb8_table();
    let c = c.clamp(0.0, 1.0);
    let mut k = start[((c * SRGB_BUCKETS as f32) as usize).min(SRGB_BUCKETS - 1)] as usize;
    while c >= steps[k + 1] {
        k += 1;
    }
    k as u32
}

/// `rects` with overlapping ones joined (their bounding rectangle), so
/// that no pixel is in two; empty ones dropped.
fn disjoint(rects: Vec<IRect>) -> Vec<IRect> {
    let mut out: Vec<IRect> = Vec::with_capacity(rects.len());
    for mut r in rects.into_iter().filter(|r| !r.is_empty()) {
        // Join with every rectangle it overlaps, until it overlaps none.
        while let Some(i) = out.iter().position(|o| !o.intersect(&r).is_empty()) {
            let o = out.swap_remove(i);
            r = IRect::new(r.x0.min(o.x0), r.y0.min(o.y0), r.x1.max(o.x1), r.y1.max(o.y1));
        }
        out.push(r);
    }
    out
}

/// How the phosphor style is drawn (`--tube`, `--vignette`, `--no-trails`,
/// `--no-shed`, `--force-phosphor`, as terse-host's).
#[derive(Clone, Copy, Default)]
pub struct Looks {
    pub look: Look,
    pub no_trails: bool,
    /// Every machine in the phosphor style (`--force-phosphor`).
    pub force_phosphor: bool,
}

/// Glyphs that draw nothing: the tube's own terminal is blank (the text's
/// light comes from the drivers' layers).
struct NoGlyphs(u32);

impl GlyphSource for NoGlyphs {
    fn cell_size(&self) -> (u32, u32) {
        (self.0, 2 * self.0)
    }
    fn rasterize(&mut self, _: char, _: u32, _: &mut [u8]) {}
    fn resized(&self, width: u32) -> Box<dyn GlyphSource> {
        Box::new(NoGlyphs(width.max(2)))
    }
}

/// A machine's phosphor tube: terse-host's screen given the shapes and the
/// drivers' light, and device 9's list as last given.
struct Tube {
    screen: SoftScreen,
    v3: Vector3,
}

/// The colour of a plate: what terse-host draws there (a blank terminal's
/// background).
fn plate_colour() -> u32 {
    let [r, g, b] = Terminal::default().palette()[0];
    (r as u32) << 16 | (g as u32) << 8 | b as u32
}

/// The columns of row `y` of a layer `w` pixels wide that hold light (an
/// alpha above zero), as x0..x1; (0, 0) if none.
fn extent(px: &[u32], w: usize, y: usize) -> (u32, u32) {
    let row = &px[y * w..(y + 1) * w];
    match row.iter().position(|&p| p >> 24 != 0) {
        None => (0, 0),
        Some(a) => (a as u32, (w - row.iter().rev().position(|&p| p >> 24 != 0).unwrap_or(0)) as u32),
    }
}

/// One machine's screen: its layers, composed into `image`, which covers
/// `area` (in the frame's coordinates, or its own for a panel).
pub struct Screen {
    /// Where it is drawn (pixels); `None`: a single machine's screen, the
    /// largest whole multiple of the native size in the viewport, the
    /// letterbox included in its area.
    rect: Option<(f32, f32, f32, f32)>,
    /// The glyph cell width it is drawn with (0: native).
    cell: u32,
    /// Its layers bottom to top: (slot, id).
    order: Vec<(u8, u8)>,
    layers: HashMap<(u8, u8), Layer>,
    /// The 3D display's layer is drawn over a scene's panels rather than
    /// among the others.
    scene: bool,
    place: Option<((u32, u32), (f32, f32, f32, f32), u32, bool, bool)>,
    pub image: Pixels,
    damage: Damage,
    /// The 3D layer changed since the scene was last put together.
    top_changed: bool,
    /// How it is shown in this draw.
    pub show: Show,
    /// The phosphor tube, in the phosphor style.
    tube: Option<Tube>,
    /// In the phosphor style, per light layer and row of the picture: the
    /// columns that hold light.
    extents: HashMap<(u8, u8), Vec<(u32, u32)>>,
    /// The area drawn as a plate, if it is one.
    plated: Option<IRect>,
    /// The last render's PRESENT and composing times (ns).
    spent: (u64, u64),
}

/// The layer of device 9 (SPEC §vector3d).
const LAYER3: (u8, u8) = (terse_core::vector3::SLOT, 0);

impl Screen {
    fn new(devices: &BusDevices) -> Self {
        Screen {
            rect: None,
            cell: 0,
            order: devices.layers.iter().map(|(s, l)| (*s, l.id)).collect(),
            layers: HashMap::new(),
            scene: false,
            place: None,
            image: Pixels::default(),
            damage: Damage::default(),
            top_changed: false,
            show: Show::default(),
            tube: None,
            extents: HashMap::new(),
            plated: None,
            spent: (0, 0),
        }
    }

    pub fn screen_rect(&self, native: (u32, u32), viewport: (u32, u32)) -> (f32, f32, f32, f32) {
        self.rect.unwrap_or_else(|| grid_rect(native, viewport))
    }

    /// The picture: the tube's in the phosphor style, else the layers'.
    pub fn image(&self) -> &Pixels {
        match &self.tube {
            Some(t) if self.phosphor() => &t.screen.image,
            _ => &self.image,
        }
    }

    /// Whether it is shown in the phosphor style.
    pub fn phosphor(&self) -> bool {
        self.show.phosphor && !self.show.plate
    }

    pub fn area(&self) -> IRect {
        self.image().area
    }

    fn take_damage(&mut self, into: &mut Damage) {
        into.merge(&self.damage);
        self.damage.clear();
    }

    /// Brings the image up to date: PRESENT to the machine's drivers for a
    /// picture of its area's size, then the layers composed where they
    /// changed (or, in the phosphor style, the tube drawn from the lists and
    /// the layers' light); a plate is drawn without its drivers.
    fn render(&mut self, devices: &mut BusDevices, native: (u32, u32), viewport: (u32, u32), pool: &Pool, eye: Eye, looks: &Looks) -> Result<(), String> {
        let rect = self.screen_rect(native, viewport);
        let full = IRect::of_size(viewport);
        let area = match self.rect {
            Some(r) => IRect::covering(r).intersect(&full),
            None => full,
        };
        if self.show.plate {
            // SPEC §machines.ink: nothing of the machine's screen is drawn.
            self.tube = None;
            self.extents.clear();
            if self.plated != Some(area) {
                self.image = Pixels::new(area);
                self.image.data.fill(plate_colour());
                self.damage = Damage::new(viewport);
                self.damage.add(area);
                self.plated = Some(area);
                self.place = None;
            }
            return Ok(());
        }
        self.plated = None;
        let light = self.show.phosphor;
        let place = (viewport, rect, self.cell, self.scene, light);
        let mut all = false;
        if self.place != Some(place) {
            if self.image.area != area {
                self.image = Pixels::new(area);
            }
            self.damage = Damage::new(viewport);
            self.place = Some(place);
            all = true;
        }
        if area.is_empty() {
            return Ok(());
        }
        let picture = (area.width() as u32, area.height() as u32);
        // In the phosphor style the screen's top left is a whole pixel, as
        // in terse-host's tube.
        let local = if light {
            ((rect.0 as i32 - area.x0) as f32, (rect.1 as i32 - area.y0) as f32, rect.2, rect.3)
        } else {
            (rect.0 - area.x0 as f32, rect.1 - area.y0 as f32, rect.2, rect.3)
        };
        let t = std::time::Instant::now();
        let updates = devices.present(picture, local, self.cell)?;
        let presented = std::time::Instant::now();
        self.spent = ((presented - t).as_nanos() as u64, 0);
        let r = self.compose_layers(&updates, picture, area, rect, local, all, native, viewport, pool, eye, looks);
        self.spent.1 = presented.elapsed().as_nanos() as u64;
        r
    }

    /// The layers' news composed into the image (or the tube drawn).
    #[allow(clippy::too_many_arguments)]
    fn compose_layers(
        &mut self,
        updates: &[LayerUpdate],
        picture: (u32, u32),
        area: IRect,
        _rect: (f32, f32, f32, f32),
        local: (f32, f32, f32, f32),
        all: bool,
        native: (u32, u32),
        viewport: (u32, u32),
        pool: &Pool,
        eye: Eye,
        looks: &Looks,
    ) -> Result<(), String> {
        let light = self.show.phosphor;
        let damage = self.take_updates(updates, picture)?;
        if light && self.show.own_tube && !self.scene {
            // DEVICES.md 7.1 with `vector.tube`: the drivers' light, the vector
            // driver's beam among it, added (linear) over black.
            self.tube = None;
            self.extents.clear();
            let mut slots = self.show.light_slots.clone();
            slots.extend([3, terse_core::vector3::SLOT]);
            let dirty: Vec<IRect> = if all { vec![IRect::of_size(picture)] } else { damage.into_iter().filter(|(k, _)| slots.contains(&k.0)).map(|(_, r)| r).collect() };
            // Each layer's damage, overlapping rectangles joined: two
            // drivers that both changed the screen's rectangle compose it
            // once.
            let dirty = disjoint(dirty);
            let stack: Vec<&[u32]> = self.order.iter().filter(|k| slots.contains(&k.0)).filter_map(|k| self.layers.get(k)).map(|l| l.pixels()).collect();
            let lin = linear_table();
            let w = picture.0 as usize;
            for r in &dirty {
                let bands = tile_rows(&mut self.image.data[r.y0 as usize * w..r.y1 as usize * w], w, *r);
                pool.each(bands, |(y0, band)| {
                    for (k, out) in band.chunks_exact_mut(w).enumerate() {
                        let row = (y0 as usize + k) * w;
                        for x in r.x0 as usize..r.x1 as usize {
                            let mut sum = [0.0f32; 3];
                            let mut n = 0;
                            let mut only = 0;
                            for l in &stack {
                                let p = l[row + x];
                                let a = p >> 24;
                                if a == 0 || p & 0xFF_FFFF == 0 {
                                    continue;
                                }
                                n += 1;
                                only = p;
                                let f = a as f32 / 255.0;
                                sum[0] += lin[(p >> 16 & 255) as usize] * f;
                                sum[1] += lin[(p >> 8 & 255) as usize] * f;
                                sum[2] += lin[(p & 255) as usize] * f;
                            }
                            out[x] = match n {
                                0 => 0,
                                // One layer's full light: its colour as it is.
                                1 if only >> 24 == 255 => only & 0xFF_FFFF,
                                _ => srgb8(sum[0]) << 16 | srgb8(sum[1]) << 8 | srgb8(sum[2]),
                            };
                        }
                    }
                });
                self.damage.add(IRect::new(r.x0 + area.x0, r.y0 + area.y0, r.x1 + area.x0, r.y1 + area.y0));
            }
            return Ok(());
        }
        if light {
            let slots = &self.show.light_slots;
            self.extents.retain(|k, _| slots.contains(&k.0));
            let (w, h) = (picture.0 as usize, picture.1 as usize);
            for (key, r) in damage.iter().filter(|(k, _)| slots.contains(&k.0)) {
                let Some(l) = self.layers.get(key) else { continue };
                let px = l.pixels();
                let rows = self.extents.entry(*key).or_default();
                if rows.len() != h {
                    *rows = (0..h).map(|y| extent(px, w, y)).collect();
                } else {
                    for y in r.y0.max(0) as usize..(r.y1.max(0) as usize).min(h) {
                        rows[y] = extent(px, w, y);
                    }
                }
            }
            self.draw_tube(native, viewport, pool, eye, looks, picture, local);
            return Ok(());
        }
        self.tube = None;
        self.extents.clear();
        let damage: Vec<IRect> = damage.into_iter().filter(|(k, _)| !(self.scene && *k == LAYER3)).map(|(_, r)| r).collect();
        let mut dirty: Vec<IRect> = if all { vec![IRect::of_size(picture)] } else { damage };
        dirty.retain(|r| !r.is_empty());
        if dirty.is_empty() {
            return Ok(());
        }
        let stack: Vec<&Layer> = self.order.iter().filter(|k| !(self.scene && **k == LAYER3)).filter_map(|k| self.layers.get(k)).collect();
        let stack: Vec<&[u32]> = stack.iter().map(|l| l.pixels()).collect();
        let w = picture.0 as usize;
        for r in &dirty {
            for y in r.y0..r.y1 {
                let row = y as usize * w;
                let out = &mut self.image.data[row..row + w];
                for x in r.x0 as usize..r.x1 as usize {
                    let mut c = 0;
                    for l in &stack {
                        let p = l[row + x];
                        if p >> 24 != 0 {
                            c = over(p, c);
                        }
                    }
                    out[x] = c;
                }
            }
            self.damage.add(IRect::new(r.x0 + area.x0, r.y0 + area.y0, r.x1 + area.x0, r.y1 + area.y0));
        }
        Ok(())
    }

    /// The phosphor style: terse-host's tube, given the vector displays'
    /// shapes (within the machine's caps) and the light layers' light, its
    /// afterglow moved on as `show.advance` says.
    #[allow(clippy::too_many_arguments)]
    fn draw_tube(&mut self, native: (u32, u32), viewport: (u32, u32), pool: &Pool, eye: Eye, looks: &Looks, picture: (u32, u32), local: (f32, f32, f32, f32)) {
        let rect = self.rect;
        let tube = self.tube.get_or_insert_with(|| {
            let mut screen = SoftScreen::new(Box::new(NoGlyphs((native.0 / COLS as u32).max(2))));
            screen.set_page_animation(false);
            Tube { screen, v3: Vector3::default() }
        });
        match self.show.list3.as_deref() {
            Some(l) if !l.is_empty() => {
                tube.v3.submit(l);
            }
            _ => tube.v3.clear(),
        }
        let s = &mut tube.screen;
        s.set_grid_size(Some(native));
        s.set_rect(rect);
        s.set_force_phosphor(true);
        s.set_no_trails(looks.no_trails);
        s.set_look(looks.look);
        let vector3 = self.show.list3.is_some().then_some(&tube.v3);
        s.update_vectors(&VectorScene { vector: self.show.vector.as_ref(), vector3, eye, caps: self.show.caps });
        s.set_excluded(&self.show.excluded);
        s.place(viewport);
        let advance = match self.show.advance {
            Some(a) => Some(a || s.needs_light()),
            None => s.needs_light().then_some(true),
        };
        if let Some(advance) = advance {
            let mut layers = Vec::new();
            let mut extents = Vec::new();
            for k in self.order.iter().filter(|k| self.show.light_slots.contains(&k.0)) {
                if let (Some(l), Some(e)) = (self.layers.get(k), self.extents.get(k)) {
                    layers.push(l.pixels());
                    extents.push(&e[..]);
                }
            }
            let ll = LightLayers {
                layers,
                width: picture.0 as usize,
                height: picture.1 as usize,
                origin: (local.0.max(0.0) as usize, local.1.max(0.0) as usize),
                extents,
            };
            s.accumulate_with(viewport, advance, pool, Some(&ll));
        }
        s.take_damage(&mut self.damage);
    }

    /// The drivers' news: new files mapped; each layer's damage (picture
    /// pixels).
    fn take_updates(&mut self, updates: &[LayerUpdate], picture: (u32, u32)) -> Result<Vec<((u8, u8), IRect)>, String> {
        let mut out = Vec::new();
        for u in updates {
            let key = (u.slot, u.id);
            if let Some(p) = &u.path
                && self.layers.get(&key).is_none_or(|l| &l.path != p)
            {
                let map = Mapping::open(p, false)?;
                if map.len() < (picture.0 * picture.1 * 4) as usize {
                    return Err(format!("slot {}: layer file {} is too small", u.slot, p.display()));
                }
                self.layers.insert(key, Layer { map, path: p.clone(), size: picture });
                self.extents.remove(&key);
            }
            let in3d = self.scene && key == LAYER3;
            for d in &u.damage {
                let r = IRect::new(d[0].max(0), d[1].max(0), d[2].min(picture.0 as i32), d[3].min(picture.1 as i32));
                if r.is_empty() {
                    continue;
                }
                if in3d {
                    self.top_changed = true;
                }
                out.push((key, r));
            }
            if in3d && u.path.is_some() {
                self.top_changed = true;
            }
        }
        Ok(out)
    }

    /// The 3D layer, to go over a scene's panels (not in the phosphor
    /// style: its scene's lines are the core's).
    fn top(&self) -> Option<LayerTop<'_>> {
        if self.phosphor() {
            return None;
        }
        let l = self.layers.get(&LAYER3)?;
        let (x0, y0) = (self.image.area.x0, self.image.area.y0);
        Some(LayerTop { px: l.pixels(), width: l.size.0 as usize, origin: (x0, y0) })
    }
}

/// A premultiplied layer over a scene's panels.
struct LayerTop<'a> {
    px: &'a [u32],
    width: usize,
    /// Where the layer's pixel (0, 0) is in the image's coordinates.
    origin: (i32, i32),
}

impl Top for LayerTop<'_> {
    fn draw(&self, band: &mut [u32], area: IRect, y0: i32, clip: IRect) {
        let stride = area.width();
        for y in clip.y0..clip.y1 {
            let line = &mut band[(y - y0) as usize * stride..][..stride];
            let ly = (y - self.origin.1) as usize;
            for x in clip.x0..clip.x1 {
                let p = self.px[ly * self.width + (x - self.origin.0) as usize];
                if p >> 24 != 0 {
                    let d = &mut line[(x - area.x0) as usize];
                    *d = over(p, *d);
                }
            }
        }
    }
}

/// A machine the shell started, as shown.
struct Child {
    handle: u8,
    serial: u32,
    screen: Screen,
    /// Shown as a panel: its picture's pyramid, made again when it changes.
    panel: Option<Option<Pyramid>>,
    scene: Option<SceneImage>,
}

impl Child {
    fn image(&self) -> &Pixels {
        self.scene.as_ref().map_or(self.screen.image(), |s| &s.image)
    }
}

/// The host's consent prompt (SPEC §content.feeds) as the core draws it: a terminal
/// of the prompt's rows drawn by terse-host's grid renderer in the
/// overlay's colours, over everything.
struct Overlay {
    screen: SoftScreen,
    cell: u32,
}

/// The picture of a system or a machine.
pub struct Compositor {
    pub shell: Screen,
    shell_scene: Option<SceneImage>,
    children: HashMap<u32, Child>,
    /// The native grid size (the terminal driver's).
    native: (u32, u32),
    pub panel_mode: PanelMode,
    pool: Pool,
    frame: Pixels,
    damage: Damage,
    eye: Eye,
    last_damage: Vec<IRect>,
    composed: (Vec<IRect>, IRect),
    overlay: Option<Overlay>,
    /// The overlay's rectangle now (empty: no prompt).
    overlay_rect: IRect,
    glyphs: Option<terse_host::glyphs::TermGlyphs>,
    /// Pixels composed (measurements).
    pub pixels_composed: u64,
    /// The host's notice (a slow device, `PERFORMANCE.md` R15): heading and
    /// two lines, drawn as the prompt is when no prompt is asked.
    pub notice: Option<(String, String, String)>,
    /// How the phosphor style is drawn.
    pub looks: Looks,
    /// The system ink is shared (SPEC §machines.ink); off only for
    /// measuring (`TERSE_SYSTEM_INK=off`, as terse-host).
    pub system_ink: bool,
    /// The machines (serials) that ran a frame since the last draw: their
    /// afterglow moves on in the next.
    ran: Vec<u32>,
    /// The next draw draws every phosphor screen's beam again without
    /// moving the afterglow on (`refresh`).
    refresh: bool,
    /// The shares of the system ink in the last draw.
    pub shares: Shares,
    /// Where the last draw's time went (measurements, `--selftest`).
    pub profile: Profile,
}

/// Where a draw's time went, in nanoseconds: the machines' properties and
/// styles (`show`), the drivers' PRESENT, the screens' layers composed (or
/// the tube drawn), and the frame put together.
#[derive(Clone, Copy, Debug, Default)]
pub struct Profile {
    pub show_ns: u64,
    pub present_ns: u64,
    pub layers_ns: u64,
    pub frame_ns: u64,
}

/// The eye as the `core.eye` property.
fn eye_bytes(e: Eye) -> Vec<u8> {
    [e.0.to_le_bytes(), e.1.to_le_bytes()].concat()
}

/// The view of the shell's scene as the `core.view` property.
fn view_bytes(v: &terse_core::vector3::SceneView) -> Vec<u8> {
    let mut out = vec![v.screens];
    for row in v.view.m {
        for x in row {
            out.extend_from_slice(&x.to_le_bytes());
        }
    }
    for x in v.view.t {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// The ink caps as the `core.caps` property: empty for the devices' own.
fn caps_bytes(c: Caps) -> Vec<u8> {
    if c == Caps::FULL {
        return Vec::new();
    }
    [c.ink2.to_le_bytes(), c.ink3.to_le_bytes()].concat()
}

/// What the terminal shows over its bottom row (`core.status`).
fn status(m: &Machine<BusDevices>) -> String {
    m.fault().map_or(String::new(), |f| format!("fault: {f}"))
}

/// Whether the system ink is shared: not with `TERSE_SYSTEM_INK=off`.
fn system_ink_on() -> bool {
    std::env::var("TERSE_SYSTEM_INK").map_or(true, |v| v != "off")
}

/// The shares of the system ink (SPEC §machines.ink): terse-core's count,
/// the 2D lists' ink from the vector driver's `vector.list`.
pub fn shares(system: &System<BusDevices>) -> Shares {
    system.shares_with(&|m, cap| m.devices().vector().map_or(0, |v| terse_core::vector::drawn_ink(v.list(), v.outlined(), cap)))
}

impl Compositor {
    /// `native`: the native grid size; `threads`: the composing threads (0:
    /// one per core).
    pub fn new(shell: &BusDevices, native: (u32, u32), threads: usize) -> Self {
        Compositor {
            shell: Screen::new(shell),
            shell_scene: None,
            children: HashMap::new(),
            native,
            panel_mode: PanelMode::Perspective,
            pool: Pool::new(threads),
            frame: Pixels::default(),
            damage: Damage::default(),
            eye: (0.0, 0.0),
            last_damage: Vec::new(),
            composed: (Vec::new(), IRect::EMPTY),
            overlay: None,
            overlay_rect: IRect::EMPTY,
            glyphs: Some(terse_host::glyphs::TermGlyphs::builtin(terse_host::FONT_PX)),
            pixels_composed: 0,
            notice: None,
            looks: Looks::default(),
            system_ink: system_ink_on(),
            ran: Vec::new(),
            refresh: false,
            shares: Shares::default(),
            profile: Profile::default(),
        }
    }

    pub fn set_eye(&mut self, eye: Eye) {
        self.eye = eye;
    }

    pub fn eye(&self) -> Eye {
        self.eye
    }

    /// The machines (serials) that ran a frame since the last draw (from
    /// the system frame's result): in the phosphor style their afterglow
    /// moves on in the next draw, as terse-host's does each frame.
    pub fn ran(&mut self, frame: &terse_core::SystemFrame, system: &System<BusDevices>) {
        if frame.shell.is_some() {
            self.ran.push(0);
        }
        for (h, _) in &frame.children {
            if let Some(c) = system.child(*h) {
                self.ran.push(c.serial);
            }
        }
    }

    /// The next draw draws the beams of what is shown now without moving
    /// the afterglow on (a screenshot's last picture, as terse-host's
    /// `refresh_beams`).
    pub fn refresh(&mut self) {
        self.refresh = true;
    }

    /// The frame drawn last.
    pub fn frame(&self) -> &Pixels {
        &self.frame
    }

    /// The rectangles of the frame drawn last that changed.
    pub fn damage(&self) -> &[IRect] {
        &self.last_damage
    }

    /// The native cell width.
    fn native_cell(&self) -> u32 {
        self.native.0 / COLS as u32
    }

    /// The shell's screen rectangle in `viewport`.
    pub fn shell_rect(&self, viewport: (u32, u32)) -> (f32, f32, f32, f32) {
        self.shell.screen_rect(self.native, viewport)
    }

    /// How a machine is shown: its style, its lists (the 3D one only with
    /// `lines`), its caps, whether a plate; and its drivers told what they
    /// need to draw it (the eye, its status, light mode, the caps).
    fn show(&self, serial: u32, d: &mut BusDevices, status: String, lines: bool, share: Option<&terse_core::system_ink::Share>, excluded: Vec<[u16; 4]>) -> Result<Show, String> {
        let plate = share.is_some_and(|s| s.plate);
        let caps = Caps::of(share);
        let vector = if plate { None } else { d.vector() };
        let phosphor = !plate && (self.looks.force_phosphor || vector.as_ref().is_some_and(Vector::phosphor));
        let advance = if self.refresh { Some(false) } else { self.ran.contains(&serial).then_some(true) };
        if !plate {
            d.set_property(property::EYE, &eye_bytes(self.eye))?;
            d.set_property(property::STATUS, status.as_bytes())?;
            d.set_property(property::LIGHT, &[phosphor as u8])?;
            d.set_property(property::CAPS, &caps_bytes(caps))?;
            // How much a driver's own look sheds (DEVICES.md 7.2): the
            // level the core's own look is shed to (R15 step 1).
            d.set_property(property::SHED, &[self.looks.look.floor.min(3)])?;
        }
        let light_slots = if phosphor { d.light_slots() } else { Vec::new() };
        let list3 = (phosphor && lines).then(|| d.list3().into_owned());
        let own_tube = phosphor && d.property(property::TUBE)?.is_some_and(|v| v.first() == Some(&1));
        Ok(Show { plate, phosphor, vector, list3, caps, excluded, advance, light_slots, own_tube })
    }

    /// Draws the picture of `system` for `viewport` after a frame, with the
    /// consent prompt (the question, how many wait, the asking machine's
    /// title) if one is asked: each machine's drivers draw what changed,
    /// then the frame is put together where anything did.
    pub fn draw(&mut self, system: &mut System<BusDevices>, viewport: (u32, u32), prompt: Option<(&Prompt, usize, String)>) -> Result<&Pixels, String> {
        let began = std::time::Instant::now();
        self.profile = Profile::default();
        let eye = self.eye;
        if system.is_system() {
            let r = system_rect(viewport);
            self.shell.rect = Some(r);
            let w = cell_width_for(r);
            // As terse-host's shell screen: its native glyphs unless the
            // cell is another size.
            self.shell.cell = if w == self.native_cell() { 0 } else { w };
        }
        let shell_rect = self.shell.screen_rect(self.native, viewport);
        // SPEC §machines.ink: each machine's share of the system's ink.
        self.shares = if self.system_ink && system.is_system() { shares(system) } else { Shares::default() };
        let shares = self.shares.clone();
        // With Screens in effect the shell's 3D lines are drawn over the
        // panels (SPEC §machines.screens); with its own Surfaces, over them (SPEC §vector3d.presentation).
        let scene = system.scene(eye);
        let mut layers: Option<SceneLayers> = scene_layers(system, eye).or_else(|| surface_layers(system.shell(), eye, terse_core::vector3::INK_CAP));
        plate_panels(&mut layers, &shares);
        let view = scene.as_ref().map_or_else(Vec::new, |s| view_bytes(&s.view));
        // The properties the drivers draw with: the eye, the view, a fault,
        // light mode.
        let shell = system.shell_mut();
        let st = status(shell);
        let lines = layers.is_none();
        let excluded = layers.as_ref().map_or_else(Vec::new, |l| l.rects.clone());
        let d = shell.devices_mut();
        d.set_property(property::VIEW, &view)?;
        self.shell.show = self.show(0, d, st, lines, None, excluded)?;
        self.shell.scene = layers.is_some();
        // The pictures shown flat and the panels, with the machines'.
        let pictures = system.pictures();
        let panels: Vec<terse_core::vector3::Panel> = layers.as_ref().map_or_else(Vec::new, |l| l.panels.iter().filter(|p| !p.is_surface()).copied().collect());
        let mut keep = Vec::new();
        for (h, p) in &pictures {
            let share = shares.of(*h).copied();
            let Some(child) = system.child_mut(*h) else { continue };
            let serial = child.serial;
            let Some(machine) = &mut child.machine else { continue };
            let rect = picture_rect(shell_rect, *p);
            let plate = share.is_some_and(|s| s.plate);
            let own = surface_layers(machine, eye, Caps::of(share.as_ref()).ink3).filter(|_| !plate);
            let st = status(machine);
            let lines = own.is_none();
            let excluded = own.as_ref().map_or_else(Vec::new, |l| l.rects.clone());
            let d = machine.devices_mut();
            let show = self.show(serial, d, st, lines, share.as_ref(), excluded)?;
            let c = self.children.entry(serial).or_insert_with(|| Child { handle: *h, serial, screen: Screen::new(d), panel: None, scene: None });
            c.panel = None;
            c.screen.rect = Some(rect);
            c.screen.cell = cell_width_for(rect);
            c.screen.scene = own.is_some();
            c.screen.show = show;
            if SceneImage::update(&mut c.scene, own) {
                self.damage.add(c.screen.area());
            }
            keep.push(serial);
        }
        let cap = (shell_rect.2 / COLS as f32).floor() as u32;
        for p in &panels {
            let share = shares.of(p.handle).copied();
            if share.is_some_and(|s| s.plate) {
                continue; // a plate: no picture to make
            }
            let Some(child) = system.child_mut(p.handle) else { continue };
            let serial = child.serial;
            let Some(machine) = &mut child.machine else { continue };
            let current = self.children.get(&serial).filter(|c| c.panel.is_some()).map(|c| c.screen.cell);
            let cw = panel_cell_width(panel_width_px(p, eye, shell_rect), cap, current);
            let size = (cw * COLS as u32, cw * COLS as u32 * 3 / 4);
            let own = surface_layers(machine, eye, Caps::of(share.as_ref()).ink3);
            let st = status(machine);
            let lines = own.is_none();
            let excluded = own.as_ref().map_or_else(Vec::new, |l| l.rects.clone());
            let d = machine.devices_mut();
            let show = self.show(serial, d, st, lines, share.as_ref(), excluded)?;
            let c = self.children.entry(serial).or_insert_with(|| Child { handle: p.handle, serial, screen: Screen::new(d), panel: None, scene: None });
            if c.panel.is_none() || current != Some(cw) {
                c.panel = Some(None);
            }
            c.screen.rect = Some((0.0, 0.0, size.0 as f32, size.1 as f32));
            c.screen.cell = cw;
            c.screen.scene = own.is_some();
            c.screen.show = show;
            if SceneImage::update(&mut c.scene, own)
                && let Some(panel) = &mut c.panel
            {
                *panel = None;
            }
            keep.push(serial);
        }
        self.children.retain(|serial, _| keep.contains(serial));
        if SceneImage::update(&mut self.shell_scene, layers) {
            self.damage.add(self.shell.area());
        }
        self.ran.clear();
        self.refresh = false;
        self.overlay_rect = self.prompt(prompt, shell_rect, viewport);
        self.profile.show_ns = began.elapsed().as_nanos() as u64;
        self.render(system, viewport)
    }

    /// The consent prompt over the shell's screen: drawn, and its
    /// rectangle (empty: none).
    fn prompt(&mut self, prompt: Option<(&Prompt, usize, String)>, rect: (f32, f32, f32, f32), viewport: (u32, u32)) -> IRect {
        use terse_host::consent::{FIRST_ROW, with_notice, with_prompt};
        let shown = match (prompt, &self.notice) {
            (Some((p, waiting, title)), _) => with_prompt(&Terminal::default(), &title, p, waiting),
            (None, Some((h, a, b))) => with_notice(&Terminal::default(), h, a, b),
            (None, None) => {
                self.overlay = None;
                return IRect::EMPTY;
            }
        };
        let Some(glyphs) = &self.glyphs else { return IRect::EMPTY };
        let cell = if self.shell.rect.is_some() { cell_width_for(rect) } else { self.native_cell() };
        if self.overlay.as_ref().is_none_or(|o| o.cell != cell) {
            let source: Box<dyn terse_host::display::GlyphSource> = Box::new(glyphs.clone());
            let mut screen = SoftScreen::new(source);
            screen.set_page_animation(false);
            self.overlay = Some(Overlay { screen, cell });
        }
        let o = self.overlay.as_mut().unwrap();
        o.screen.set_rect(self.shell.rect);
        o.screen.update(&shown);
        o.screen.update_page(None, None);
        o.screen.set_overlay(Some(FIRST_ROW));
        o.screen.place(viewport);
        o.screen.render(viewport, &self.pool);
        o.screen.overlay_rect(viewport)
    }

    /// Brings every screen up to date and puts the frame together where
    /// anything changed.
    fn render(&mut self, system: &mut System<BusDevices>, viewport: (u32, u32)) -> Result<&Pixels, String> {
        if self.frame.area != IRect::of_size(viewport) {
            self.frame = Pixels::new(IRect::of_size(viewport));
            self.damage = Damage::new(viewport);
            self.damage.add_all();
            self.composed = (Vec::new(), IRect::EMPTY);
        }
        let (eye, mode, native, looks) = (self.eye, self.panel_mode, self.native, self.looks);
        let none = HashMap::new();
        self.shell.render(system.shell_mut().devices_mut(), native, viewport, &self.pool, eye, &looks)?;
        let mut spent = self.shell.spent;
        let mut shell_damage = Damage::new(viewport);
        self.shell.take_damage(&mut shell_damage);
        // Each child's screen; panels in their own images, with a new
        // pyramid when they changed.
        let mut panels_changed = false;
        let mut by_serial: HashMap<u32, &mut Machine<BusDevices>> = system
            .children_mut()
            .filter_map(|(_, c)| {
                let serial = c.serial;
                Some((serial, c.machine.as_mut()?))
            })
            .collect();
        for (serial, c) in self.children.iter_mut() {
            let Some(m) = by_serial.get_mut(serial) else { continue };
            let Some(panel) = &mut c.panel else { continue };
            let s = &mut c.screen;
            let size = (s.rect.unwrap().2 as u32, s.rect.unwrap().3 as u32);
            s.render(m.devices_mut(), native, size, &self.pool, eye, &looks)?;
            spent = (spent.0 + s.spent.0, spent.1 + s.spent.1);
            let mut d = Damage::new(size);
            s.take_damage(&mut d);
            let mut changed = !d.is_empty();
            if let Some(scene) = &mut c.scene {
                if std::mem::take(&mut s.top_changed) {
                    scene.stale = true;
                }
                let top = s.top();
                changed = scene.refresh(s.image(), changed, &none, s.screen_rect(native, size), eye, mode, &self.pool, top.as_ref().map(|t| t as &dyn Top));
            }
            if changed || panel.is_none() {
                let image = c.scene.as_ref().map_or(s.image(), |sc| &sc.image);
                *panel = Some(Pyramid::build(image));
                panels_changed = true;
            }
        }
        // The shell's scene: its image, the panels and its 3D layer.
        match &mut self.shell_scene {
            None => self.damage.merge(&shell_damage),
            Some(scene) => {
                let rect = self.shell.screen_rect(native, viewport);
                let pyramids: HashMap<u8, &Pyramid> =
                    self.children.values().filter_map(|c| Some((c.handle, c.panel.as_ref()?.as_ref()?))).collect();
                if panels_changed || std::mem::take(&mut self.shell.top_changed) {
                    scene.stale = true;
                }
                let top = self.shell.top();
                if scene.refresh(self.shell.image(), !shell_damage.is_empty(), &pyramids, rect, eye, mode, &self.pool, top.as_ref().map(|t| t as &dyn Top)) {
                    self.damage.add(scene.image.area);
                }
            }
        }
        let mut order: Vec<&mut Child> = self.children.values_mut().filter(|c| c.panel.is_none()).collect();
        order.sort_by_key(|c| c.handle);
        for c in order.iter_mut() {
            let Some(m) = by_serial.get_mut(&c.serial) else { continue };
            let s = &mut c.screen;
            s.render(m.devices_mut(), native, viewport, &self.pool, eye, &looks)?;
            spent = (spent.0 + s.spent.0, spent.1 + s.spent.1);
            match &mut c.scene {
                None => s.take_damage(&mut self.damage),
                Some(scene) => {
                    let mut d = Damage::new(viewport);
                    s.take_damage(&mut d);
                    if std::mem::take(&mut s.top_changed) {
                        scene.stale = true;
                    }
                    let top = s.top();
                    if scene.refresh(s.image(), !d.is_empty(), &none, s.screen_rect(native, viewport), eye, mode, &self.pool, top.as_ref().map(|t| t as &dyn Top)) {
                        self.damage.add(scene.image.area);
                    }
                }
            }
        }
        self.profile.present_ns = spent.0;
        self.profile.layers_ns = spent.1;
        let framed = std::time::Instant::now();
        // Pictures that came, went or moved, and the overlay.
        let areas: Vec<IRect> = order.iter().map(|c| c.screen.area()).collect();
        let overlay = self.overlay_rect;
        if (&areas, overlay) != (&self.composed.0, self.composed.1) {
            for r in areas.iter().chain(&self.composed.0) {
                self.damage.add(*r);
            }
            self.damage.add(overlay);
            self.damage.add(self.composed.1);
            self.composed = (areas, overlay);
        } else if self.overlay.is_some() {
            // The prompt may have changed: its rows again.
            self.damage.add(overlay);
        }
        self.last_damage = self.damage.rects();
        if self.damage.is_empty() {
            self.profile.frame_ns = framed.elapsed().as_nanos() as u64;
            return Ok(&self.frame);
        }
        // Put the frame together, a tile row per thread.
        let bottom = self.shell_scene.as_ref().map_or(self.shell.image(), |s| &s.image);
        let screens: Vec<&Pixels> = std::iter::once(bottom).chain(order.iter().map(|c| c.image())).collect();
        let top = self.overlay.as_ref().map(|o| &o.screen.image);
        let rows = self.damage.rows();
        let stride = self.frame.stride();
        let area = self.frame.area;
        for (_, runs) in &rows {
            for r in runs {
                self.pixels_composed += (r.width() * r.height()) as u64;
            }
        }
        let mut bands: Vec<Option<(i32, &mut [u32])>> = tile_rows(&mut self.frame.data, stride, area).into_iter().map(Some).collect();
        let work: Vec<(i32, &mut [u32], Vec<IRect>)> =
            rows.into_iter().filter_map(|(ty, runs)| bands.get_mut(ty)?.take().map(|(y0, band)| (y0, band, runs))).collect();
        self.pool.each(work, |(y0, band, runs)| {
            for run in runs {
                for y in run.y0..run.y1 {
                    let line = &mut band[(y - y0) as usize * stride..][..stride];
                    line[run.x0 as usize..run.x1 as usize].fill(0);
                    for img in &screens {
                        let a = img.area.intersect(&run);
                        if a.is_empty() || y < a.y0 || y >= a.y1 {
                            continue;
                        }
                        line[a.x0 as usize..a.x1 as usize].copy_from_slice(img.row(y, a.x0, a.x1));
                    }
                    if let Some(img) = top {
                        let o = overlay.intersect(&run);
                        if !o.is_empty() && y >= o.y0 && y < o.y1 {
                            line[o.x0 as usize..o.x1 as usize].copy_from_slice(img.row(y, o.x0, o.x1));
                        }
                    }
                }
            }
        });
        self.damage.clear();
        self.profile.frame_ns = framed.elapsed().as_nanos() as u64;
        Ok(&self.frame)
    }
}

/// RGBA bytes of an XRGB picture (for a PNG).
pub fn rgba(frame: &Pixels) -> Vec<u8> {
    frame.to_rgba()
}

/// Writes RGBA pixels to a PNG file.
pub fn write_png(path: &Path, rgba: &[u8], (w, h): (u32, u32)) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb8_by_table_is_the_function() {
        let (steps, _) = srgb8_table();
        // Every step and its neighbours, and a fine sweep.
        for k in 1..256 {
            let s = steps[k];
            for c in [s, f32::from_bits(s.to_bits() - 1), f32::from_bits(s.to_bits() + 1)] {
                assert_eq!(srgb8(c), srgb8_exact(c), "{c}");
            }
        }
        for i in 0..=200_000 {
            let c = i as f32 / 200_000.0;
            assert_eq!(srgb8(c), srgb8_exact(c), "{c}");
        }
        for c in [-1.0, 0.0, 1.0, 2.0, f32::NAN.max(0.0)] {
            assert_eq!(srgb8(c), srgb8_exact(c), "{c}");
        }
    }

    #[test]
    fn overlapping_damage_is_joined() {
        let r = |a, b, c, d| IRect::new(a, b, c, d);
        assert_eq!(disjoint(vec![r(0, 0, 10, 10), r(0, 0, 10, 10)]), vec![r(0, 0, 10, 10)]);
        assert_eq!(disjoint(vec![r(0, 0, 10, 10), r(20, 0, 30, 10)]).len(), 2);
        let mut j = disjoint(vec![r(0, 0, 10, 10), r(20, 0, 30, 10), r(5, 5, 25, 6)]);
        j.sort_by_key(|r| r.x0);
        assert_eq!(j, vec![r(0, 0, 30, 10)]);
        assert!(disjoint(vec![r(0, 0, 0, 10)]).is_empty());
    }
}
