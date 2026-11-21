//! The core's compositor (DEVICES.md 7): the picture of a system (SPEC
//! §machines.screens) or of a single machine, put together from the drivers' layers
//! as terse-host's software renderer puts its screens together (the clean
//! style): each machine's layers composed into its screen's image at the
//! size it is shown at (the drivers draw each machine at that size), the
//! shell's image at the bottom, the children's pictures over it in handle
//! order, a scene's panels (Screens of other machines, a machine's own
//! Surfaces) and its 3D layer over them, and the host's consent prompt
//! over everything. The scene, the panels and their pyramids are
//! terse-host's (`soft::scene`, `soft::panel`); the frame is XRGB8888
//! (softbuffer's and DRM's format).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use terse_core::{COLS, Machine, System, Terminal};
use terse_host::display::{
    Eye, SceneLayers, cell_width_for, grid_rect, panel_cell_width, panel_width_px, picture_rect, scene_layers, surface_layers, system_rect,
};
use terse_host::soft::base::{Damage, IRect, Pixels, Pool, tile_rows};
use terse_host::soft::panel::{PanelMode, Pyramid};
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
    place: Option<((u32, u32), (f32, f32, f32, f32), u32, bool)>,
    pub image: Pixels,
    damage: Damage,
    /// The 3D layer changed since the scene was last put together.
    top_changed: bool,
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
        }
    }

    pub fn screen_rect(&self, native: (u32, u32), viewport: (u32, u32)) -> (f32, f32, f32, f32) {
        self.rect.unwrap_or_else(|| grid_rect(native, viewport))
    }

    pub fn area(&self) -> IRect {
        self.image.area
    }

    fn take_damage(&mut self, into: &mut Damage) {
        into.merge(&self.damage);
        self.damage.clear();
    }

    /// Brings the image up to date: PRESENT to the machine's drivers for a
    /// picture of its area's size, then the layers composed where they
    /// changed.
    fn render(&mut self, devices: &mut BusDevices, native: (u32, u32), viewport: (u32, u32)) -> Result<(), String> {
        let rect = self.screen_rect(native, viewport);
        let full = IRect::of_size(viewport);
        let area = match self.rect {
            Some(r) => IRect::covering(r).intersect(&full),
            None => full,
        };
        let place = (viewport, rect, self.cell, self.scene);
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
        let local = (rect.0 - area.x0 as f32, rect.1 - area.y0 as f32, rect.2, rect.3);
        let updates = devices.present(picture, local, self.cell)?;
        let damage = self.take_updates(&updates, picture)?;
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

    /// The drivers' news: new files mapped; the damage (picture pixels).
    fn take_updates(&mut self, updates: &[LayerUpdate], picture: (u32, u32)) -> Result<Vec<IRect>, String> {
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
            }
            let in3d = self.scene && key == LAYER3;
            for d in &u.damage {
                let r = IRect::new(d[0].max(0), d[1].max(0), d[2].min(picture.0 as i32), d[3].min(picture.1 as i32));
                if r.is_empty() {
                    continue;
                }
                if in3d {
                    self.top_changed = true;
                } else {
                    out.push(r);
                }
            }
            if in3d && u.path.is_some() {
                self.top_changed = true;
            }
        }
        Ok(out)
    }

    /// The 3D layer, to go over a scene's panels.
    fn top(&self) -> Option<LayerTop<'_>> {
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
        self.scene.as_ref().map_or(&self.screen.image, |s| &s.image)
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

/// What the terminal shows over its bottom row (`core.status`).
fn status(m: &Machine<BusDevices>) -> String {
    m.fault().map_or(String::new(), |f| format!("fault: {f}"))
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
        }
    }

    pub fn set_eye(&mut self, eye: Eye) {
        self.eye = eye;
    }

    pub fn eye(&self) -> Eye {
        self.eye
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

    /// Draws the picture of `system` for `viewport` after a frame, with the
    /// consent prompt (the question, how many wait, the asking machine's
    /// title) if one is asked: each machine's drivers draw what changed,
    /// then the frame is put together where anything did.
    pub fn draw(&mut self, system: &mut System<BusDevices>, viewport: (u32, u32), prompt: Option<(&Prompt, usize, String)>) -> Result<&Pixels, String> {
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
        // With Screens in effect the shell's 3D lines are drawn over the
        // panels (SPEC §machines.screens); with its own Surfaces, over them (SPEC §vector3d.presentation).
        let scene = system.scene(eye);
        let layers: Option<SceneLayers> = scene_layers(system, eye).or_else(|| surface_layers(system.shell(), eye));
        let view = scene.as_ref().map_or_else(Vec::new, |s| view_bytes(&s.view));
        // The properties the drivers draw with: the eye, the view, a fault.
        let shell = system.shell_mut();
        let st = status(shell);
        let d = shell.devices_mut();
        d.set_property(property::EYE, &eye_bytes(eye))?;
        d.set_property(property::VIEW, &view)?;
        d.set_property(property::STATUS, st.as_bytes())?;
        self.shell.scene = layers.is_some();
        // The pictures shown flat and the panels, with the machines'.
        let pictures = system.pictures();
        let panels: Vec<terse_core::vector3::Panel> = layers.as_ref().map_or_else(Vec::new, |l| l.panels.iter().filter(|p| !p.is_surface()).copied().collect());
        let mut keep = Vec::new();
        for (h, p) in &pictures {
            let Some(child) = system.child_mut(*h) else { continue };
            let serial = child.serial;
            let Some(machine) = &mut child.machine else { continue };
            let rect = picture_rect(shell_rect, *p);
            let own = surface_layers(machine, eye);
            let st = status(machine);
            let d = machine.devices_mut();
            d.set_property(property::EYE, &eye_bytes(eye))?;
            d.set_property(property::STATUS, st.as_bytes())?;
            let c = self.children.entry(serial).or_insert_with(|| Child { handle: *h, serial, screen: Screen::new(d), panel: None, scene: None });
            c.panel = None;
            c.screen.rect = Some(rect);
            c.screen.cell = cell_width_for(rect);
            c.screen.scene = own.is_some();
            if SceneImage::update(&mut c.scene, own) {
                self.damage.add(c.screen.area());
            }
            keep.push(serial);
        }
        let cap = (shell_rect.2 / COLS as f32).floor() as u32;
        for p in &panels {
            let Some(child) = system.child_mut(p.handle) else { continue };
            let serial = child.serial;
            let Some(machine) = &mut child.machine else { continue };
            let current = self.children.get(&serial).filter(|c| c.panel.is_some()).map(|c| c.screen.cell);
            let cw = panel_cell_width(panel_width_px(p, eye, shell_rect), cap, current);
            let size = (cw * COLS as u32, cw * COLS as u32 * 3 / 4);
            let own = surface_layers(machine, eye);
            let st = status(machine);
            let d = machine.devices_mut();
            d.set_property(property::EYE, &eye_bytes(eye))?;
            d.set_property(property::STATUS, st.as_bytes())?;
            let c = self.children.entry(serial).or_insert_with(|| Child { handle: p.handle, serial, screen: Screen::new(d), panel: None, scene: None });
            if c.panel.is_none() || current != Some(cw) {
                c.panel = Some(None);
            }
            c.screen.rect = Some((0.0, 0.0, size.0 as f32, size.1 as f32));
            c.screen.cell = cw;
            c.screen.scene = own.is_some();
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
        self.overlay_rect = self.prompt(prompt, shell_rect, viewport);
        self.render(system, viewport)
    }

    /// The consent prompt over the shell's screen: drawn, and its
    /// rectangle (empty: none).
    fn prompt(&mut self, prompt: Option<(&Prompt, usize, String)>, rect: (f32, f32, f32, f32), viewport: (u32, u32)) -> IRect {
        use terse_host::consent::{FIRST_ROW, with_prompt};
        let Some((p, waiting, title)) = prompt else {
            self.overlay = None;
            return IRect::EMPTY;
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
        o.screen.update(&with_prompt(&Terminal::default(), &title, p, waiting));
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
        let (eye, mode, native) = (self.eye, self.panel_mode, self.native);
        let none = HashMap::new();
        self.shell.render(system.shell_mut().devices_mut(), native, viewport)?;
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
            s.render(m.devices_mut(), native, size)?;
            let mut d = Damage::new(size);
            s.take_damage(&mut d);
            let mut changed = !d.is_empty();
            if let Some(scene) = &mut c.scene {
                if std::mem::take(&mut s.top_changed) {
                    scene.stale = true;
                }
                let top = s.top();
                changed = scene.refresh(&s.image, changed, &none, s.screen_rect(native, size), eye, mode, &self.pool, top.as_ref().map(|t| t as &dyn Top));
            }
            if changed || panel.is_none() {
                let image = c.scene.as_ref().map_or(&s.image, |sc| &sc.image);
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
                if scene.refresh(&self.shell.image, !shell_damage.is_empty(), &pyramids, rect, eye, mode, &self.pool, top.as_ref().map(|t| t as &dyn Top)) {
                    self.damage.add(scene.image.area);
                }
            }
        }
        let mut order: Vec<&mut Child> = self.children.values_mut().filter(|c| c.panel.is_none()).collect();
        order.sort_by_key(|c| c.handle);
        for c in order.iter_mut() {
            let Some(m) = by_serial.get_mut(&c.serial) else { continue };
            let s = &mut c.screen;
            s.render(m.devices_mut(), native, viewport)?;
            match &mut c.scene {
                None => s.take_damage(&mut self.damage),
                Some(scene) => {
                    let mut d = Damage::new(viewport);
                    s.take_damage(&mut d);
                    if std::mem::take(&mut s.top_changed) {
                        scene.stale = true;
                    }
                    let top = s.top();
                    if scene.refresh(&s.image, !d.is_empty(), &none, s.screen_rect(native, viewport), eye, mode, &self.pool, top.as_ref().map(|t| t as &dyn Top)) {
                        self.damage.add(scene.image.area);
                    }
                }
            }
        }
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
            return Ok(&self.frame);
        }
        // Put the frame together, a tile row per thread.
        let bottom = self.shell_scene.as_ref().map_or(&self.shell.image, |s| &s.image);
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
