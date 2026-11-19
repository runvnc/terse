//! The screens of a system's machines drawn together (SPEC 12.3): the
//! shell's screen over the whole window, then each machine's picture in its
//! rectangle, in increasing handle order, each a complete screen with its
//! own layers, glyphs rasterised for its size and its own phosphor tube;
//! then the host's consent prompt over everything. A single machine is the
//! shell's screen alone, drawn exactly as before.

use std::collections::HashMap;
use std::sync::Arc;

use terse_core::{Machine, System};
use terse_net::Prompt;

use crate::display::{Display, ShownPrompt, picture_rect, system_rect};
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
            self.shell.set_rect(device, Some(system_rect(viewport)));
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
        let shell_rect = self.shell.screen_rect(viewport);
        let mut keep = Vec::new();
        for (h, p) in system.pictures() {
            let Some(child) = system.child(h) else { continue };
            let Some(machine) = &child.machine else { continue };
            let rect = picture_rect(shell_rect, p);
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

/// The GPU renderer as a [`Display`]: the screens with their device, and an
/// offscreen target for screenshots and timing.
pub struct GpuDisplay {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub screens: Screens,
    format: wgpu::TextureFormat,
    adapter: String,
    offscreen: Option<((u32, u32), wgpu::Texture)>,
}

impl GpuDisplay {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, screens: Screens, format: wgpu::TextureFormat, adapter: String) -> Self {
        GpuDisplay { device, queue, screens, format, adapter, offscreen: None }
    }

    /// Renders into an offscreen texture of `viewport` pixels (made once
    /// per size) and returns it.
    fn render_target(&mut self, viewport: (u32, u32)) -> wgpu::Texture {
        if self.offscreen.as_ref().is_none_or(|(size, _)| *size != viewport) {
            let target = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("offscreen"),
                size: wgpu::Extent3d { width: viewport.0, height: viewport.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            self.offscreen = Some((viewport, target));
        }
        self.offscreen.as_ref().unwrap().1.clone()
    }
}

impl Display for GpuDisplay {
    fn update(
        &mut self,
        system: &System,
        viewport: (u32, u32),
        ran: &[u32],
        layout: &dyn Fn(u32) -> Option<Arc<HostLayout>>,
        prompt: ShownPrompt,
    ) {
        self.screens.update(&self.device, &self.queue, system, viewport, ran, layout, prompt);
    }

    fn configure(&mut self, no_trails: bool, force_phosphor: bool, animate_page: bool) {
        self.screens.set_no_trails(no_trails);
        self.screens.set_force_phosphor(force_phosphor);
        self.screens.set_page_animation(animate_page);
    }

    fn refresh_beams(&mut self, viewport: (u32, u32)) {
        self.screens.refresh_beams(&self.device, &self.queue, viewport);
    }

    fn fade_shell(&mut self, viewport: (u32, u32)) {
        self.screens.shell.accumulate(&self.device, &self.queue, viewport);
    }

    fn shell_phosphor(&self) -> bool {
        self.screens.shell.phosphor()
    }

    fn anchor(&self, serial: u32, x: u16, y: u16) -> u16 {
        self.screens.anchor(serial, x, y)
    }

    fn screen_pos(&self, viewport: (u32, u32), x: f64, y: f64) -> (u16, u16) {
        self.screens.shell.screen_pos(viewport, x, y)
    }

    fn row_height(&self, viewport: (u32, u32)) -> f64 {
        self.screens.shell.row_height(viewport)
    }

    fn take_page_figure_log(&mut self) -> Vec<String> {
        self.screens.shell.take_page_figure_log()
    }

    fn snapshot(&mut self, viewport: (u32, u32)) -> Result<Vec<u8>, String> {
        let (width, height) = viewport;
        let target = self.render_target(viewport);
        let row_bytes = width * 4;
        let padded = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        self.screens.render(&self.device, &self.queue, &mut encoder, &view, viewport);
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: None },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback buffer"));
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| e.to_string())?;
        let mapped = slice.get_mapped_range().map_err(|e| e.to_string())?;
        Ok(mapped.chunks(padded as usize).flat_map(|row| &row[..row_bytes as usize]).copied().collect())
    }

    fn render_offscreen(&mut self, viewport: (u32, u32)) -> Result<(), String> {
        let target = self.render_target(viewport);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        self.screens.render(&self.device, &self.queue, &mut encoder, &view, viewport);
        self.queue.submit([encoder.finish()]);
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn grid_size(&self) -> (u32, u32) {
        self.screens.shell.grid_size()
    }

    fn name(&self) -> String {
        format!("gpu, {}", self.adapter)
    }
}
