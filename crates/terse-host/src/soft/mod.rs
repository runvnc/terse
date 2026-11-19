//! The software renderer: the whole picture of a system (SPEC 12.3) drawn
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
//! Pixels are XRGB8888 (`0x00RRGGBB` words), the format of softbuffer and of
//! DRM dumb buffers, so presenting is a copy.

pub mod base;
mod page;
mod phosphor;

pub use phosphor::BloomRes;
mod screen;
mod shapes;

use std::collections::HashMap;
use std::sync::Arc;

use terse_core::{Machine, System};

use self::base::{Damage, IRect, Pixels, Pool, tile_rows};
use self::screen::SoftScreen;
use crate::display::{Display, Eye, GlyphSource, ShownPrompt, VectorScene, cell_width_for, picture_rect, system_rect};
use crate::page::HostLayout;
use crate::{consent, displayed_terminal};

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

pub struct SoftScreens {
    pub shell: SoftScreen,
    /// The children shown, by serial: handle and screen.
    children: HashMap<u32, (u8, SoftScreen)>,
    no_trails: bool,
    force_phosphor: bool,
    animate_page: bool,
    bloom: BloomRes,
    pool: Pool,
    frame: Pixels,
    damage: Damage,
    /// The eye for 3D lists, the same for every machine (SPEC 13.1).
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
            no_trails: false,
            force_phosphor: false,
            animate_page: true,
            bloom: BloomRes::Auto,
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

    /// At which resolution the phosphor bloom is computed (`--bloom`).
    pub fn set_bloom(&mut self, bloom: BloomRes) {
        self.bloom = bloom;
        self.shell.set_bloom(bloom);
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
        let pool = &self.pool;
        self.shell.render(viewport, pool);
        self.shell.take_damage(&mut self.damage);
        let mut order: Vec<&mut (u8, SoftScreen)> = self.children.values_mut().collect();
        order.sort_by_key(|(h, _)| *h);
        for (_, s) in order.iter_mut() {
            s.render(viewport, pool);
            s.take_damage(&mut self.damage);
        }
        // Pictures that came, went or moved, and the overlay.
        let areas: Vec<IRect> = order.iter().map(|(_, s)| s.area()).collect();
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
        let screens: Vec<&Pixels> = std::iter::once(&self.shell.image).chain(order.iter().map(|(_, s)| &s.image)).collect();
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
        show(&mut self.shell, shell, layout(0), eye);
        self.shell.place(viewport);
        if ran.contains(&0) {
            self.shell.accumulate(viewport, true, &self.pool);
        }
        let shell_rect = self.shell.screen_rect(viewport);
        let mut keep = Vec::new();
        for (h, p) in system.pictures() {
            let Some(child) = system.child(h) else { continue };
            let Some(machine) = &child.machine else { continue };
            let rect = picture_rect(shell_rect, p);
            let fresh = !self.children.contains_key(&child.serial);
            let (no_trails, force, animate, bloom) = (self.no_trails, self.force_phosphor, self.animate_page, self.bloom);
            let shell_s = &self.shell;
            let (_, s) = self.children.entry(child.serial).or_insert_with(|| {
                let mut s = SoftScreen::new(shell_s.glyphs_for(cell_width_for(rect)));
                s.set_no_trails(no_trails);
                s.set_force_phosphor(force);
                s.set_page_animation(animate);
                s.set_bloom(bloom);
                (h, s)
            });
            s.set_rect(Some(rect));
            s.update(&displayed_terminal(machine));
            show(s, machine, layout(child.serial), eye);
            s.place(viewport);
            if fresh || ran.contains(&child.serial) {
                s.accumulate(viewport, true, &self.pool);
            }
            keep.push(child.serial);
        }
        self.children.retain(|serial, _| keep.contains(serial));
    }

    fn set_eye(&mut self, eye: Eye) {
        self.eye = eye;
    }

    fn configure(&mut self, no_trails: bool, force_phosphor: bool, animate_page: bool) {
        self.set_no_trails(no_trails);
        self.set_force_phosphor(force_phosphor);
        self.set_page_animation(animate_page);
    }

    fn refresh_beams(&mut self, viewport: (u32, u32)) {
        self.shell.accumulate(viewport, false, &self.pool);
        let pool = &self.pool;
        let mut order: Vec<&mut (u8, SoftScreen)> = self.children.values_mut().collect();
        order.sort_by_key(|(h, _)| *h);
        for (_, s) in order {
            s.accumulate(viewport, false, pool);
        }
    }

    fn fade_shell(&mut self, viewport: (u32, u32)) {
        self.shell.accumulate(viewport, true, &self.pool);
    }

    fn shell_phosphor(&self) -> bool {
        self.shell.phosphor()
    }

    fn anchor(&self, serial: u32, x: u16, y: u16) -> u16 {
        let s = if serial == 0 { Some(&self.shell) } else { self.children.get(&serial).map(|(_, s)| s) };
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

/// A machine's vector list and page to its screen.
fn show(s: &mut SoftScreen, machine: &Machine, layout: Option<Arc<HostLayout>>, eye: Eye) {
    s.update_vectors(&VectorScene { vector: machine.vector(), vector3: machine.vector3(), eye });
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
        s.update_vectors(&crate::display::VectorScene { vector: Some(v), vector3: None, eye: (0.0, 0.0) });
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
            s.update_vectors(&VectorScene { vector: Some(&v), vector3: Some(v3), eye });
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
        s.update_vectors(&crate::display::VectorScene { vector: Some(&v), vector3: None, eye: (0.0, 0.0) });
        s.update_page(None, None);
        s.accumulate(VIEWPORT, true, &pool);
        let lit = |s: &SoftScreen| s.image.data.iter().filter(|&&p| p != 0).count();
        let first = lit(&s);
        assert!(first > 0);
        v.clear();
        t.command(0x01);
        t.command(0x06);
        s.update(&t);
        s.update_vectors(&crate::display::VectorScene { vector: Some(&v), vector3: None, eye: (0.0, 0.0) });
        s.accumulate(VIEWPORT, true, &pool);
        let fading = lit(&s);
        assert!(fading > 0, "an afterglow");
        for _ in 0..200 {
            s.accumulate(VIEWPORT, true, &pool);
        }
        assert_eq!(lit(&s), 0, "all dark again");
    }
}
