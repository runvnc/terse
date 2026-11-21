//! The software renderer: the whole picture of a system (SPEC §machines.screens) drawn
//! by the CPU into a pixel buffer, the same picture the GPU renderer
//! (`screens.rs`, `renderer.rs`) draws, for machines without a usable GPU
//! and for the bootable image, which shows the buffer on a KMS/DRM display
//! through a [`FrameSink`].
//!
//! Each machine's screen is a [`SoftScreen`] with an image of the pixels it
//! covers, brought up to date where it changed (`screen.rs`; the phosphor
//! tube in `phosphor.rs`, shapes in `shapes.rs`, the page in `page.rs`).
//! The frame is then put together where anything changed: the shell's
//! image, each machine's picture over it in increasing handle order, the
//! host's prompt over everything. The frame's damage (tiles of
//! [`base::TILE`] pixels) is kept for the presenter, so an idle system
//! costs almost nothing per frame and a presenter can copy only what
//! changed.
//!
//! A machine with a 3D scene over its glass (the shell's Screens, any
//! machine's own Surfaces, SPEC §machines.screens, §vector3d.presentation) is put together in an image of
//! its own when anything of it changed: its screen's image with the
//! Surfaces' sources blacked out, the panels far to near (a Surface from a
//! pyramid of that same image, before the sources were blacked out), its
//! 3D lines over them.
//!
//! Pixels are XRGB8888 (`0x00RRGGBB` words), the format of softbuffer and of
//! DRM dumb buffers, so presenting is a copy.

pub mod base;
pub mod page;
pub mod panel;
#[cfg(test)]
mod look;
mod phosphor;
pub mod scene;

pub mod screen;
pub mod shapes;

use std::collections::HashMap;
use std::sync::Arc;

use terse_core::{Machine, System};

use self::base::{Damage, IRect, Pixels, Pool, tile_rows};
use self::panel::{PanelMode, Pyramid};
use self::scene::SceneImage;
use self::screen::SoftScreen;
use crate::display::{
    Display, Eye, GlyphSource, ShownPrompt, VectorScene, cell_width_for, panel_cell_width, panel_width_px,
    picture_rect, scene_layers, surface_layers, system_rect,
};
use crate::page::HostLayout;
use crate::{consent, displayed_terminal, shown_terminal};
use crate::display::Caps;
use terse_core::system_ink::Share;

/// Where finished frames go: a window (softbuffer), or later a KMS/DRM
/// display of the bootable image.
#[allow(dead_code)]
pub trait FrameSink {
    /// The size frames should have, in pixels.
    fn size(&self) -> (u32, u32);
    /// Shows `frame`; `damage` lists the rectangles that changed since the
    /// frame presented before (all of it after a resize).
    fn present(&mut self, frame: &Pixels, damage: &[IRect]) -> Result<(), String>;
}

/// A machine shown as a panel: its picture's pyramid, made again when its
/// image changes.
struct PanelImage {
    pyramid: Option<Pyramid>,
}

/// A machine the shell started, as shown: its handle and screen, its
/// pyramid if it is shown as a panel, its own scene if it has Surfaces.
struct Child {
    handle: u8,
    screen: SoftScreen,
    panel: Option<PanelImage>,
    scene: Option<SceneImage>,
}

impl Child {
    /// The image that shows it: its scene's, or its screen's.
    fn image(&self) -> &Pixels {
        self.scene.as_ref().map_or(&self.screen.image, |s| &s.image)
    }
}

pub struct SoftScreens {
    pub shell: SoftScreen,
    /// The children shown, by serial.
    children: HashMap<u32, Child>,
    /// The shell's 3D scene, when Screens or its own Surfaces are in effect.
    scene: Option<SceneImage>,
    /// How panels are drawn (`--panels`).
    panel_mode: PanelMode,
    no_trails: bool,
    force_phosphor: bool,
    animate_page: bool,
    look: crate::phosphor::Look,
    pool: Pool,
    frame: Pixels,
    damage: Damage,
    /// The eye for 3D lists, the same for every machine (SPEC §vector3d.space).
    eye: Eye,
    /// The damage of the frame drawn last, for presenters.
    last_damage: Vec<IRect>,
    /// What the frame was put together from: the children's areas in
    /// order, and the overlay's rectangle.
    composed: (Vec<IRect>, IRect),
}

impl SoftScreens {
    /// `threads`: the workers that draw (0: one per core).
    pub fn new(glyphs: Box<dyn GlyphSource>, threads: usize) -> Self {
        SoftScreens {
            shell: SoftScreen::new(glyphs),
            children: HashMap::new(),
            scene: None,
            panel_mode: PanelMode::Perspective,
            no_trails: false,
            force_phosphor: false,
            animate_page: true,
            look: crate::phosphor::Look::default(),
            pool: Pool::new(threads),
            frame: Pixels::default(),
            damage: Damage::default(),
            eye: (0.0, 0.0),
            last_damage: Vec::new(),
            composed: (Vec::new(), IRect::EMPTY),
        }
    }

    pub fn threads(&self) -> usize {
        self.pool.threads()
    }

    /// How panels are drawn: in perspective, or flat (`--panels flat`).
    pub fn set_panel_mode(&mut self, mode: PanelMode) {
        if mode != self.panel_mode {
            self.panel_mode = mode;
            for s in self.scene.iter_mut().chain(self.children.values_mut().filter_map(|c| c.scene.as_mut())) {
                s.stale = true;
            }
        }
    }

    pub fn set_no_trails(&mut self, on: bool) {
        self.no_trails = on;
        self.shell.set_no_trails(on);
    }

    pub fn set_force_phosphor(&mut self, on: bool) {
        self.force_phosphor = on;
        self.shell.set_force_phosphor(on);
    }

    pub fn set_page_animation(&mut self, on: bool) {
        self.animate_page = on;
        self.shell.set_page_animation(on);
    }

    /// How the phosphor style is drawn (`--tube`, `--vignette`).
    pub fn set_look(&mut self, look: crate::phosphor::Look) {
        self.look = look;
        self.shell.set_look(look);
        for c in self.children.values_mut() {
            c.screen.set_look(look);
        }
    }

    /// Brings the frame up to date for `viewport`: each screen draws what
    /// changed, then the frame is put together where anything did.
    pub fn render(&mut self, viewport: (u32, u32)) -> &Pixels {
        if self.frame.area != IRect::of_size(viewport) {
            self.frame = Pixels::new(IRect::of_size(viewport));
            self.damage = Damage::new(viewport);
            self.damage.add_all();
            self.composed = (Vec::new(), IRect::EMPTY);
        }
        let (pool, eye, mode) = (&self.pool, self.eye, self.panel_mode);
        let none = HashMap::new();
        self.shell.render(viewport, pool);
        let mut shell_damage = Damage::new(viewport);
        self.shell.take_damage(&mut shell_damage);
        // Panels: each machine in its own image (with its own scene); a
        // new pyramid when it changed.
        let mut panels_changed = false;
        for c in self.children.values_mut() {
            let Some(panel) = &mut c.panel else { continue };
            let s = &mut c.screen;
            let size = s.area();
            let own = (size.width() as u32, size.height() as u32);
            s.render(own, pool);
            let mut d = Damage::new(own);
            s.take_damage(&mut d);
            let mut changed = !d.is_empty();
            if let Some(scene) = &mut c.scene {
                changed = scene.refresh(&s.image, changed, &none, s.screen_rect(own), eye, mode, pool, None);
            }
            if changed || panel.pyramid.is_none() {
                let image = c.scene.as_ref().map_or(&s.image, |sc| &sc.image);
                panel.pyramid = Some(Pyramid::build(image));
                panels_changed = true;
            }
        }
        // The shell's scene: its image, the panels and the lines, again
        // where anything of them changed.
        match &mut self.scene {
            None => self.damage.merge(&shell_damage),
            Some(scene) => {
                let rect = self.shell.screen_rect(viewport);
                let pyramids: HashMap<u8, &Pyramid> =
                    self.children.values().filter_map(|c| Some((c.handle, c.panel.as_ref()?.pyramid.as_ref()?))).collect();
                if panels_changed {
                    scene.stale = true;
                }
                if scene.refresh(&self.shell.image, !shell_damage.is_empty(), &pyramids, rect, eye, mode, pool, None) {
                    self.damage.add(scene.image.area);
                }
            }
        }
        let mut order: Vec<&mut Child> = self.children.values_mut().filter(|c| c.panel.is_none()).collect();
        order.sort_by_key(|c| c.handle);
        for c in order.iter_mut() {
            let s = &mut c.screen;
            s.render(viewport, pool);
            match &mut c.scene {
                None => s.take_damage(&mut self.damage),
                Some(scene) => {
                    let mut d = Damage::new(viewport);
                    s.take_damage(&mut d);
                    if scene.refresh(&s.image, !d.is_empty(), &none, s.screen_rect(viewport), eye, mode, pool, None) {
                        self.damage.add(scene.image.area);
                    }
                }
            }
        }
        // Pictures that came, went or moved, and the overlay.
        let areas: Vec<IRect> = order.iter().map(|c| c.screen.area()).collect();
        let overlay = self.shell.overlay_rect(viewport);
        if (&areas, overlay) != (&self.composed.0, self.composed.1) {
            for r in areas.iter().chain(&self.composed.0) {
                self.damage.add(*r);
            }
            self.damage.add(overlay);
            self.damage.add(self.composed.1);
            self.composed = (areas, overlay);
        }
        self.last_damage = self.damage.rects();
        if self.damage.is_empty() {
            return &self.frame;
        }
        // Put the frame together, a tile row per thread.
        let bottom = self.scene.as_ref().map_or(&self.shell.image, |s| &s.image);
        let screens: Vec<&Pixels> = std::iter::once(bottom).chain(order.iter().map(|c| c.image())).collect();
        let shell = &self.shell.image;
        let rows = self.damage.rows();
        let stride = self.frame.stride();
        let area = self.frame.area;
        let mut bands: Vec<Option<(i32, &mut [u32])>> = tile_rows(&mut self.frame.data, stride, area).into_iter().map(Some).collect();
        let work: Vec<(i32, &mut [u32], Vec<IRect>)> =
            rows.into_iter().filter_map(|(ty, runs)| bands.get_mut(ty)?.take().map(|(y0, band)| (y0, band, runs))).collect();
        pool.each(work, |(y0, band, runs)| {
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
                    let o = overlay.intersect(&run);
                    if !o.is_empty() && y >= o.y0 && y < o.y1 {
                        line[o.x0 as usize..o.x1 as usize].copy_from_slice(shell.row(y, o.x0, o.x1));
                    }
                }
            }
        });
        self.damage.clear();
        &self.frame
    }

    /// The frame drawn last.
    pub fn frame(&self) -> &Pixels {
        &self.frame
    }

    /// The rectangles of the frame drawn last that changed.
    #[allow(dead_code)]
    pub fn damage(&self) -> &[IRect] {
        &self.last_damage
    }

    /// Draws the frame and hands it to `sink`.
    #[allow(dead_code)]
    pub fn present(&mut self, sink: &mut dyn FrameSink) -> Result<(), String> {
        let viewport = sink.size();
        self.render(viewport);
        sink.present(&self.frame, &self.last_damage)
    }
}

impl Display for SoftScreens {
    fn update(
        &mut self,
        system: &System,
        viewport: (u32, u32),
        ran: &[u32],
        layout: &dyn Fn(u32) -> Option<Arc<HostLayout>>,
        prompt: ShownPrompt,
    ) {
        let shell = system.shell();
        if system.is_system() {
            self.shell.set_rect(Some(system_rect(viewport)));
        }
        let shown = displayed_terminal(shell);
        match &prompt {
            Some((p, waiting, title)) => {
                self.shell.update(&consent::with_prompt(&shown, title, p, *waiting));
                self.shell.set_overlay(Some(consent::FIRST_ROW));
            }
            None => {
                self.shell.update(&shown);
                self.shell.set_overlay(None);
            }
        }
        let eye = self.eye;
        // With Screens in effect the shell's 3D lines are drawn over the
        // panels, not in its own screen (SPEC §machines.screens); with its own
        // Surfaces, over them (SPEC §vector3d.presentation).
        // SPEC §machines.ink: each machine's share of the system's ink.
        let shares = crate::system_shares(system);
        let mut layers = scene_layers(system, eye).or_else(|| surface_layers(shell, eye, terse_core::vector3::INK_CAP));
        crate::display::plate_panels(&mut layers, &shares);
        show(&mut self.shell, shell, layout(0), eye, layers.is_none(), None);
        self.shell.set_excluded(layers.as_ref().map_or(&[][..], |l| &l.rects));
        self.shell.place(viewport);
        if ran.contains(&0) {
            self.shell.accumulate(viewport, true, &self.pool);
        }
        let shell_rect = self.shell.screen_rect(viewport);
        let mut keep = Vec::new();
        let (no_trails, force, animate, look) = (self.no_trails, self.force_phosphor, self.animate_page, self.look);
        let fresh_screen = |shell_s: &SoftScreen, width: u32| {
            let mut s = SoftScreen::new(shell_s.glyphs_for(width));
            s.set_no_trails(no_trails);
            s.set_force_phosphor(force);
            s.set_page_animation(animate);
            s.set_look(look);
            s
        };
        for (h, p) in system.pictures() {
            let share = shares.of(h).copied();
            let Some(child) = system.child(h) else { continue };
            let Some(machine) = &child.machine else { continue };
            let rect = picture_rect(shell_rect, p);
            let fresh = !self.children.contains_key(&child.serial);
            let shell_s = &self.shell;
            let c = self.children.entry(child.serial).or_insert_with(|| Child {
                handle: h,
                screen: fresh_screen(shell_s, cell_width_for(rect)),
                panel: None,
                scene: None,
            });
            c.panel = None;
            let own = surface_layers(machine, eye, Caps::of(share.as_ref()).ink3).filter(|_| !share.is_some_and(|s| s.plate));
            let s = &mut c.screen;
            s.set_rect(Some(rect));
            s.update(&shown_terminal(machine, share.as_ref()));
            show(s, machine, layout(child.serial), eye, own.is_none(), share.as_ref());
            s.set_excluded(own.as_ref().map_or(&[][..], |l| &l.rects));
            s.place(viewport);
            if fresh || ran.contains(&child.serial) {
                s.accumulate(viewport, true, &self.pool);
            }
            if SceneImage::update(&mut c.scene, own) {
                self.damage.add(c.screen.area());
            }
            keep.push(child.serial);
        }
        // The panels: each machine drawn at a size its panel needs.
        if let Some(layers) = &layers {
            let cap = (shell_rect.2 / terse_core::COLS as f32).floor() as u32;
            for p in layers.panels.iter().filter(|p| !p.is_surface()) {
                let Some(child) = system.child(p.handle) else { continue };
                let share = shares.of(p.handle).copied();
                if share.is_some_and(|s| s.plate) {
                    continue; // drawn as a plate: no picture to make
                }
                let Some(machine) = &child.machine else { continue };
                let fresh = !self.children.contains_key(&child.serial);
                let current = self.children.get(&child.serial).filter(|c| c.panel.is_some()).map(|c| c.screen.screen_rect((0, 0)).2 as u32 / terse_core::COLS as u32);
                let cw = panel_cell_width(panel_width_px(p, eye, shell_rect), cap, current);
                let size = (cw * terse_core::COLS as u32, cw * terse_core::COLS as u32 * 3 / 4);
                let shell_s = &self.shell;
                let c = self.children.entry(child.serial).or_insert_with(|| Child {
                    handle: p.handle,
                    screen: fresh_screen(shell_s, cw),
                    panel: None,
                    scene: None,
                });
                let resized = c.panel.is_none() || current != Some(cw);
                if c.panel.is_none() {
                    c.panel = Some(PanelImage { pyramid: None });
                }
                let own = surface_layers(machine, eye, Caps::of(share.as_ref()).ink3).filter(|_| !share.is_some_and(|s| s.plate));
                let s = &mut c.screen;
                s.set_rect(Some((0.0, 0.0, size.0 as f32, size.1 as f32)));
                s.update(&shown_terminal(machine, share.as_ref()));
                show(s, machine, layout(child.serial), eye, own.is_none(), share.as_ref());
                s.set_excluded(own.as_ref().map_or(&[][..], |l| &l.rects));
                s.place(size);
                if fresh || resized || ran.contains(&child.serial) {
                    s.accumulate(size, true, &self.pool);
                }
                if SceneImage::update(&mut c.scene, own)
                    && let Some(panel) = &mut c.panel
                {
                    panel.pyramid = None;
                }
                keep.push(child.serial);
            }
        }
        self.children.retain(|serial, _| keep.contains(serial));
        // The scene is drawn again when what it shows changed.
        if SceneImage::update(&mut self.scene, layers) {
            self.damage.add(self.shell.area());
        }
    }

    fn set_eye(&mut self, eye: Eye) {
        self.eye = eye;
    }

    fn set_look(&mut self, look: crate::phosphor::Look) {
        SoftScreens::set_look(self, look);
    }

    fn configure(&mut self, no_trails: bool, force_phosphor: bool, animate_page: bool) {
        self.set_no_trails(no_trails);
        self.set_force_phosphor(force_phosphor);
        self.set_page_animation(animate_page);
    }

    fn refresh_beams(&mut self, viewport: (u32, u32)) {
        self.shell.accumulate(viewport, false, &self.pool);
        let pool = &self.pool;
        let mut order: Vec<&mut Child> = self.children.values_mut().collect();
        order.sort_by_key(|c| c.handle);
        for c in order {
            let own = c.screen.area();
            let size = if c.panel.is_some() { (own.width() as u32, own.height() as u32) } else { viewport };
            c.screen.accumulate(size, false, pool);
        }
    }

    fn fade_shell(&mut self, viewport: (u32, u32)) {
        self.shell.accumulate(viewport, true, &self.pool);
    }

    fn shell_phosphor(&self) -> bool {
        self.shell.phosphor()
    }

    fn anchor(&self, serial: u32, x: u16, y: u16) -> u16 {
        let s = if serial == 0 { Some(&self.shell) } else { self.children.get(&serial).map(|c| &c.screen) };
        s.map_or(0, |s| s.page_anchor_at_units(x as f32 + 0.5, y as f32 + 0.5))
    }

    fn screen_pos(&self, viewport: (u32, u32), x: f64, y: f64) -> (u16, u16) {
        self.shell.screen_pos(viewport, x, y)
    }

    fn row_height(&self, viewport: (u32, u32)) -> f64 {
        self.shell.row_height(viewport)
    }

    fn take_page_figure_log(&mut self) -> Vec<String> {
        self.shell.take_page_figure_log()
    }

    fn snapshot(&mut self, viewport: (u32, u32)) -> Result<Vec<u8>, String> {
        Ok(self.render(viewport).to_rgba())
    }

    fn render_offscreen(&mut self, viewport: (u32, u32)) -> Result<(), String> {
        self.render(viewport);
        Ok(())
    }

    fn grid_size(&self) -> (u32, u32) {
        self.shell.grid_size()
    }

    fn name(&self) -> String {
        format!("software, {} thread{}", self.threads(), if self.threads() == 1 { "" } else { "s" })
    }
}

/// A machine's vector list and page to its screen; its 3D list only with
/// `lines` (drawn over the panels when Screens or Surfaces are in effect).
/// `share`: a machine's share of the system's ink (SPEC §machines.ink): its
/// lists' caps, or a plate, nothing of its screen drawn.
fn show(s: &mut SoftScreen, machine: &Machine, layout: Option<Arc<HostLayout>>, eye: Eye, lines: bool, share: Option<&Share>) {
    let caps = Caps::of(share);
    if share.is_some_and(|s| s.plate) {
        s.update_vectors(&VectorScene { vector: None, vector3: None, eye, caps });
        s.update_page(None, None);
        return;
    }
    s.update_vectors(&VectorScene { vector: machine.vector(), vector3: machine.vector3().filter(|_| lines), eye, caps });
    let layout = layout.filter(|_| machine.page().is_some_and(|p| p.document().is_some()));
    s.update_page(machine.page(), layout);
}

#[cfg(test)]
mod tests {
    use terse_core::{Terminal, Vector};

    use super::base::{Damage, IRect, Pool};
    use super::screen::SoftScreen;
    use crate::display::GlyphSource;

    /// Glyphs without fonts: each character a block whose size depends on
    /// its code, so different characters differ.
    struct Blocks(u32);

    impl GlyphSource for Blocks {
        fn cell_size(&self) -> (u32, u32) {
            (self.0, 2 * self.0)
        }
        fn rasterize(&mut self, ch: char, cells: u32, out: &mut [u8]) {
            let (w, h) = (self.0 * cells, 2 * self.0);
            let k = ch as u32 % 5;
            for y in k..h - 1 {
                for x in 1..w - 1 - k.min(w - 2) {
                    out[(y * w + x) as usize] = (40 * k + 60) as u8;
                }
            }
        }
        fn resized(&self, width: u32) -> Box<dyn GlyphSource> {
            Box::new(Blocks(width))
        }
    }

    const VIEWPORT: (u32, u32) = (700, 500);

    /// Brings `s` up to date and draws it; returns the damage.
    fn show(s: &mut SoftScreen, t: &Terminal, v: &Vector, pool: &Pool) -> Damage {
        s.update(t);
        s.update_vectors(&crate::display::VectorScene { vector: Some(v), vector3: None, eye: (0.0, 0.0), caps: crate::display::Caps::FULL });
        s.update_page(None, None);
        s.render(VIEWPORT, pool);
        let mut d = Damage::new(VIEWPORT);
        s.take_damage(&mut d);
        d
    }

    /// What a fresh screen draws for the same state.
    fn fresh(t: &Terminal, v: &Vector, pool: &Pool) -> Vec<u32> {
        let mut s = SoftScreen::new(Box::new(Blocks(8)));
        show(&mut s, t, v, pool);
        s.image.data.clone()
    }

    /// A screen brought up to date bit by bit draws exactly what a fresh
    /// screen draws from the same state, and an idle screen draws nothing.
    #[test]
    fn incremental_drawing_matches_a_full_redraw() {
        let pool = Pool::new(2);
        let (mut t, mut v) = (Terminal::default(), Vector::default());
        let mut s = SoftScreen::new(Box::new(Blocks(8)));
        t.write("Hello, 你好 world".as_bytes());
        assert_eq!(show(&mut s, &t, &v, &pool).rects(), vec![IRect::new(0, 0, 700, 500)], "first: everything");
        assert!(show(&mut s, &t, &v, &pool).is_empty(), "idle: nothing");
        // One character: about its cell.
        t.set_cursor(30, 20);
        t.set_colours(3, 4);
        t.write(b"x");
        let d = show(&mut s, &t, &v, &pool);
        assert!(!d.is_empty() && d.rects().iter().all(|r| r.width() * r.height() <= 64 * 64), "{:?}", d.rects());
        assert_eq!(s.image.data, fresh(&t, &v, &pool));
        // Shapes come, move and go; the palette changes.
        let shapes = |x: u8| [&[0x01, 200, 40, 40, 160][..], &[0x04, x, 10, 0, 10], &[0x07, 0, 3], &[0x05, 0, 20, 0, 20], &[0x08, 0, 5, 0, 6]].concat();
        for x in [0u8, 40, 80] {
            v.submit(&shapes(x));
            let d = show(&mut s, &t, &v, &pool);
            assert!(d.rects() != vec![IRect::new(0, 0, 700, 500)]);
            assert_eq!(s.image.data, fresh(&t, &v, &pool), "shapes at {x}");
        }
        v.set_fill(1);
        show(&mut s, &t, &v, &pool);
        assert_eq!(s.image.data, fresh(&t, &v, &pool), "outlines");
        v.clear();
        show(&mut s, &t, &v, &pool);
        assert_eq!(s.image.data, fresh(&t, &v, &pool), "no shapes");
        t.palette_set(4, &[10, 200, 30]);
        show(&mut s, &t, &v, &pool);
        assert_eq!(s.image.data, fresh(&t, &v, &pool), "palette");
        assert!(show(&mut s, &t, &v, &pool).is_empty());
    }

    /// SPEC §terminal.text: a machine without full text is drawn with the
    /// basic repertoire, every other character (each half of a wide one
    /// included) as the one placeholder; with full text as the glyphs.
    #[test]
    fn basic_text_draws_placeholders() {
        let pool = Pool::new(2);
        let v = Vector::default();
        let draw = |full: bool, text: &str| {
            let mut t = Terminal::with_full_text(full);
            t.write(text.as_bytes());
            fresh(&t, &v, &pool)
        };
        // Without full text, 你 (two cells), €€ and ■‖ look the same, and
        // unlike full text's 你; basic characters are drawn as glyphs.
        let basic_wide = draw(false, "a你é");
        assert_eq!(basic_wide, draw(false, "a€€é"));
        assert_eq!(basic_wide, draw(false, "a\u{25A0}\u{2016}é"));
        assert_ne!(basic_wide, draw(true, "a你é"));
        assert_eq!(draw(false, "a─é"), draw(true, "a─é"), "box drawing is basic");
        assert_ne!(basic_wide, draw(false, "a  é"), "a placeholder is drawn");
        // The punctuation shared with articles and U+FFFD are basic: drawn,
        // not boxed. U+25A0, just past the block elements, is boxed.
        let placeholder = draw(false, "a€é");
        for c in ['…', '“', '−', '\u{FFFD}'] {
            let text = format!("a{c}é");
            assert_eq!(draw(false, &text), draw(true, &text), "{c} is basic");
            assert_ne!(draw(false, &text), placeholder, "{c} is drawn");
        }
        assert_eq!(draw(false, "a\u{25A0}é"), placeholder, "■ is a placeholder");
        assert_ne!(draw(true, "a\u{25A0}é"), placeholder);
        // An incremental screen that switches machines redraws.
        let mut s = SoftScreen::new(Box::new(Blocks(8)));
        let (mut a, mut b) = (Terminal::with_full_text(true), Terminal::with_full_text(false));
        a.write("ab你".as_bytes());
        b.write("ab你".as_bytes());
        show(&mut s, &a, &v, &pool);
        show(&mut s, &b, &v, &pool);
        let f = fresh(&b, &v, &pool);
        let diff: Vec<usize> = s.image.data.iter().zip(&f).enumerate().filter(|(_, (x, y))| x != y).map(|(i, _)| i).collect();
        assert!(diff.is_empty(), "{} differ, first at {:?}, last {:?}", diff.len(), diff.first().map(|i| (i % 700, i / 700)), diff.last().map(|i| (i % 700, i / 700)));
    }

    /// A wide glyph in column 0 replaced by narrow ones (and the other way
    /// round) is redrawn as from scratch, with no stale pixels left.
    #[test]
    fn wide_glyph_in_column_0_replaced() {
        let pool = Pool::new(2);
        let v = Vector::default();
        for (first, then) in [("你x", "ab"), ("你x", "  "), ("ab", "你"), ("你", "好")] {
            let mut s = SoftScreen::new(Box::new(Blocks(8)));
            let mut t = Terminal::with_full_text(true);
            t.write(first.as_bytes());
            show(&mut s, &t, &v, &pool);
            let mut t2 = Terminal::with_full_text(true);
            t2.write(then.as_bytes());
            show(&mut s, &t2, &v, &pool);
            let f = fresh(&t2, &v, &pool);
            let diff: Vec<(usize, usize)> = s.image.data.iter().zip(&f).enumerate().filter(|(_, (x, y))| x != y).map(|(i, _)| (i % 700, i / 700)).collect();
            assert!(diff.is_empty(), "{first:?} then {then:?}: {} differ, first at {:?}", diff.len(), diff.first());
        }
        // When the glyph atlas starts over, a slot means another glyph: "ab"
        // is drawn (slots 1 and 2), screens of new glyphs are taken but not
        // drawn until the atlas is nearly full, and "ba" then starts it
        // over, b in slot 1 and a in slot 2. The cells' slots are those
        // drawn, the glyphs are not, and both must be redrawn.
        use terse_core::terminal::{COLS, ROWS};
        let grid = (COLS * ROWS) as u32;
        let mut codes = (0x100u32..0xFFFE)
            .filter(|&c| !(0xD800..0xE000).contains(&c) && !terse_core::wide::is_wide(c as u16))
            .map(|c| c as u16);
        let mut s = SoftScreen::new(Box::new(Blocks(8)));
        let mut t = Terminal::with_full_text(true);
        t.write(b"ab");
        show(&mut s, &t, &v, &pool);
        let fillers = (super::screen::ATLAS_SLOTS - 2 * grid - 3) / grid + 1;
        for _ in 0..fillers {
            let mut f = Terminal::with_full_text(true);
            let cells: Vec<u8> = (0..grid).flat_map(|_| {
                let [lo, hi] = codes.next().unwrap().to_le_bytes();
                [lo, hi, 7, 0]
            }).collect();
            f.cells_write(0, &cells);
            s.update(&f);
        }
        let mut t = Terminal::with_full_text(true);
        t.write(b"ba");
        show(&mut s, &t, &v, &pool);
        assert_eq!(s.image.data, fresh(&t, &v, &pool), "after the atlas starts over");
    }

    /// The 3D display's segments are drawn bit by bit as from scratch too,
    /// when the list changes and when only the eye moves; an idle screen
    /// with a 3D list draws nothing.
    #[test]
    fn incremental_drawing_with_a_3d_list() {
        use crate::display::VectorScene;
        use terse_core::Vector3;
        let pool = Pool::new(2);
        let t = Terminal::default();
        let v = Vector::default();
        let mut v3 = Vector3::default();
        let show3 = |s: &mut SoftScreen, v3: &Vector3, eye: (f32, f32)| {
            s.update(&t);
            s.update_vectors(&VectorScene { vector: Some(&v), vector3: Some(v3), eye, caps: crate::display::Caps::FULL });
            s.update_page(None, None);
            s.render(VIEWPORT, &pool);
            let mut d = Damage::new(VIEWPORT);
            s.take_damage(&mut d);
            d
        };
        let fresh3 = |v3: &Vector3, eye: (f32, f32)| {
            let mut s = SoftScreen::new(Box::new(Blocks(8)));
            show3(&mut s, v3, eye);
            s.image.data.clone()
        };
        // A square in depth and a circle (a point, lines, butt-ended chords).
        let xyz = |x: i16, y: i16, z: i16| [x.to_le_bytes(), y.to_le_bytes(), z.to_le_bytes()].concat();
        let list = |z: i16| {
            [
                vec![0x01, 255, 200, 100, 255, 0x03],
                xyz(1600, 1600, 0),
                vec![0x04],
                xyz(4800, 1600, z),
                vec![0x04],
                xyz(4800, 4800, z),
                vec![0x05, 0x06, 0x00, 0x04],
            ]
            .concat()
        };
        let mut s = SoftScreen::new(Box::new(Blocks(8)));
        show3(&mut s, &v3, (0.0, 0.0));
        for (z, eye) in [(0, (0.0, 0.0)), (8000, (0.0, 0.0)), (8000, (160.0, -40.0)), (-4000, (-160.0, 160.0))] {
            v3.submit(&list(z));
            show3(&mut s, &v3, eye);
            assert_eq!(s.image.data, fresh3(&v3, eye), "z {z}, eye {eye:?}");
            assert!(show3(&mut s, &v3, eye).is_empty(), "idle: nothing");
        }
        v3.clear();
        show3(&mut s, &v3, (0.0, 0.0));
        assert_eq!(s.image.data, fresh3(&v3, (0.0, 0.0)));
    }

    /// The phosphor tube: light where the shapes and glyphs are, and an
    /// afterglow that fades all the way to black once they are gone.
    #[test]
    fn phosphor_light_fades_to_black() {
        let pool = Pool::new(1);
        let (mut t, mut v) = (Terminal::default(), Vector::default());
        t.write(b"glow");
        v.submit(&[&[0x04, 0, 10, 0, 10][..], &[0x05, 0, 30, 0, 20]].concat());
        v.set_style(1);
        v.set_persistence(200);
        let mut s = SoftScreen::new(Box::new(Blocks(8)));
        s.update(&t);
        s.update_vectors(&crate::display::VectorScene { vector: Some(&v), vector3: None, eye: (0.0, 0.0), caps: crate::display::Caps::FULL });
        s.update_page(None, None);
        s.accumulate(VIEWPORT, true, &pool);
        let lit = |s: &SoftScreen| s.image.data.iter().filter(|&&p| p != 0).count();
        let first = lit(&s);
        assert!(first > 0);
        v.clear();
        t.command(0x01);
        t.command(0x06);
        s.update(&t);
        s.update_vectors(&crate::display::VectorScene { vector: Some(&v), vector3: None, eye: (0.0, 0.0), caps: crate::display::Caps::FULL });
        s.accumulate(VIEWPORT, true, &pool);
        let fading = lit(&s);
        assert!(fading > 0, "an afterglow");
        for _ in 0..200 {
            s.accumulate(VIEWPORT, true, &pool);
        }
        assert_eq!(lit(&s), 0, "all dark again");
    }
}
