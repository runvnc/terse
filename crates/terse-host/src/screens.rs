//! The screens of a system's machines drawn together (SPEC 12.3): the
//! shell's screen over the whole window, then each machine's picture in its
//! rectangle, in increasing handle order, each a complete screen with its
//! own layers, glyphs rasterised for its size and its own phosphor tube;
//! then the host's consent prompt over everything. A single machine is the
//! shell's screen alone, drawn exactly as before.

use std::collections::HashMap;
use std::sync::Arc;

use terse_core::{Machine, SCREEN_WIDTH, System};
use terse_net::Prompt;

use crate::page::HostLayout;
use crate::renderer::Renderer;
use crate::{consent, displayed_terminal};

pub struct Screens {
    /// The shell's screen (or the single machine's).
    pub shell: Renderer,
    /// The children shown, by serial: handle and renderer.
    children: HashMap<u32, (u8, Renderer)>,
    format: wgpu::TextureFormat,
    no_trails: bool,
    force_phosphor: bool,
    animate_page: bool,
}

impl Screens {
    pub fn new(shell: Renderer, format: wgpu::TextureFormat) -> Self {
        Screens { shell, children: HashMap::new(), format, no_trails: false, force_phosphor: false, animate_page: true }
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

    /// The page anchor under (x, y) in machine `serial`'s screen units, as
    /// its page was last shown (SPEC 11.5).
    pub fn anchor(&self, serial: u32, x: u16, y: u16) -> u16 {
        let r = if serial == 0 { Some(&self.shell) } else { self.children.get(&serial).map(|(_, r)| r) };
        r.map_or(0, |r| r.page_anchor_at_units(x as f32 + 0.5, y as f32 + 0.5))
    }

    /// Takes what every machine shows after a frame: the shell's screen
    /// (with the consent prompt if one is asked: the prompt, how many wait,
    /// and the asking machine's title) and each child's picture. Machines in
    /// `ran` ran a frame: their phosphor light advances. `layout` gives the
    /// page layout to show for a serial.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        system: &System,
        viewport: (u32, u32),
        ran: &[u32],
        layout: &dyn Fn(u32) -> Option<Arc<HostLayout>>,
        prompt: Option<(&Prompt, usize, String)>,
    ) {
        let shell = system.shell();
        if system.is_system() {
            // The shell's screen fills the window (at 4:3), with glyphs
            // rasterised for that size, rather than the largest whole
            // multiple of the native grid.
            let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
            let scale = (vw / 640.0).min(vh / 480.0);
            let (w, h) = ((640.0 * scale).floor(), (480.0 * scale).floor());
            self.shell.set_rect(device, Some((((vw - w) / 2.0).floor(), ((vh - h) / 2.0).floor(), w, h)));
        }
        let shown = displayed_terminal(shell);
        match &prompt {
            Some((p, waiting, title)) => {
                self.shell.update(queue, &consent::with_prompt(&shown, title, p, *waiting));
                self.shell.set_overlay(Some(consent::FIRST_ROW));
            }
            None => {
                self.shell.update(queue, &shown);
                self.shell.set_overlay(None);
            }
        }
        show(&mut self.shell, queue, shell, layout(0));
        // Every screen's phosphor light of this frame, in one submission
        // (none if no screen is in the phosphor style).
        let mut encoder = None;
        let mut light = |r: &mut Renderer| {
            if r.phosphor() {
                let e = encoder.get_or_insert_with(|| {
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("phosphor") })
                });
                r.accumulate_into(device, queue, e, viewport);
            }
        };
        if ran.contains(&0) {
            light(&mut self.shell);
        }

        // The children's pictures, in the shell's units, to pixels.
        let (ox, oy, w, _) = self.shell.screen_rect(viewport);
        let scale = w / SCREEN_WIDTH as f32;
        let mut keep = Vec::new();
        for (h, p) in system.pictures() {
            let Some(child) = system.child(h) else { continue };
            let Some(machine) = &child.machine else { continue };
            let x0 = (ox + p.x as f32 * scale).round();
            let y0 = (oy + p.y as f32 * scale).round();
            let x1 = (ox + (p.x + p.w) as f32 * scale).round();
            let y1 = (oy + (p.y + p.h) as f32 * scale).round();
            let rect = (x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0));
            let fresh = !self.children.contains_key(&child.serial);
            let (no_trails, force, animate) = (self.no_trails, self.force_phosphor, self.animate_page);
            let (format, shell_r) = (self.format, &self.shell);
            let (_, r) = self.children.entry(child.serial).or_insert_with(|| {
                let width = ((rect.2 / terse_core::COLS as f32).floor() as u32).max(2);
                let mut r = Renderer::new(device, format, shell_r.glyphs_for(width));
                r.set_clears(false);
                r.set_no_trails(no_trails);
                r.set_force_phosphor(force);
                r.set_page_animation(animate);
                (h, r)
            });
            r.set_rect(device, Some(rect));
            r.update(queue, &displayed_terminal(machine));
            show(r, queue, machine, layout(child.serial));
            if fresh || ran.contains(&child.serial) {
                light(r);
            }
            keep.push(child.serial);
        }
        if let Some(e) = encoder {
            queue.submit([e.finish()]);
        }
        // Hidden or gone: freed.
        self.children.retain(|serial, _| keep.contains(serial));
    }

    /// Redraws every phosphor beam from what is shown now, without
    /// advancing the afterglow (a screenshot's last picture).
    pub fn refresh_beams(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, viewport: (u32, u32)) {
        self.shell.refresh_beam(device, queue, viewport);
        for (_, r) in self.children.values_mut() {
            r.refresh_beam(device, queue, viewport);
        }
    }

    /// Records the whole picture into `encoder`.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: (u32, u32),
    ) {
        self.shell.render(device, queue, encoder, target, viewport);
        let mut order: Vec<_> = self.children.values_mut().collect();
        order.sort_by_key(|(h, _)| *h);
        for (_, r) in order {
            r.render(device, queue, encoder, target, viewport);
        }
        self.shell.render_overlay(encoder, target, viewport);
    }
}

/// Uploads a machine's vector list and page to its renderer.
fn show(r: &mut Renderer, queue: &wgpu::Queue, machine: &Machine, layout: Option<Arc<HostLayout>>) {
    r.update_vectors(queue, machine.vector());
    let layout = layout.filter(|_| machine.page().is_some_and(|p| p.document().is_some()));
    r.update_page(machine.page(), layout);
}
