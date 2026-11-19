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

use crate::display::{
    Display, Eye, SceneLayers, ShownPrompt, VectorScene, eye_point, panel_cell_width, panel_width_px, picture_rect, scene_layers,
    system_rect,
};
use crate::page::HostLayout;
use crate::panels::{PANEL_FORMAT, PanelPipeline, PanelTexture};
use crate::renderer::Renderer;
use crate::vectors::VectorLayer;
use crate::{consent, displayed_terminal};

/// A machine the shell started, as shown: flat, or as a panel with its
/// texture, which is drawn again when the machine ran a frame.
struct Shown {
    handle: u8,
    renderer: Renderer,
    panel: Option<(PanelTexture, bool)>,
    /// What the machine showed when its panel was drawn last, and how
    /// many frames more to draw it anyway (the page's scroll animation
    /// settles over a few frames).
    shown: (u64, u8),
}

/// Frames a panel is drawn again after its machine's screen changed.
const SETTLE: u8 = 30;

/// A digest of what a machine's screen shows: its terminal, lists, page
/// and the page's layout; the same digest, the same picture (but for the
/// phosphor style's afterglow and the page's scroll animation).
fn digest(machine: &Machine, layout: &Option<Arc<HostLayout>>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let t = displayed_terminal(machine);
    t.cell_bytes().hash(&mut h);
    t.palette().hash(&mut h);
    (t.cursor(), t.cursor_visible()).hash(&mut h);
    machine.vector().map(|v| (v.generation(), v.phosphor())).hash(&mut h);
    machine.vector3().map(|v| v.generation()).hash(&mut h);
    machine.page().map(|p| (p.generation(), p.scroll(), p.region().x, p.region().y, p.region().w, p.region().h, p.background())).hash(&mut h);
    layout.as_ref().map(|l| Arc::as_ptr(l) as usize).hash(&mut h);
    h.finish()
}

pub struct Screens {
    /// The shell's screen (or the single machine's).
    pub shell: Renderer,
    /// The children shown, by serial.
    children: HashMap<u32, Shown>,
    /// The shell's 3D scene when Screens are in effect (SPEC 12.3), and
    /// what draws it: the panels' pipeline and the shell's 3D lines over
    /// them.
    scene: Option<SceneLayers>,
    panel_pipeline: Option<PanelPipeline>,
    lines: Option<VectorLayer>,
    format: wgpu::TextureFormat,
    no_trails: bool,
    force_phosphor: bool,
    animate_page: bool,
    /// The eye for 3D lists, the same for every machine (SPEC 13.1).
    eye: Eye,
}

impl Screens {
    pub fn new(shell: Renderer, format: wgpu::TextureFormat) -> Self {
        Screens {
            shell,
            children: HashMap::new(),
            scene: None,
            panel_pipeline: None,
            lines: None,
            format,
            no_trails: false,
            force_phosphor: false,
            animate_page: true,
            eye: (0.0, 0.0),
        }
    }

    pub fn set_eye(&mut self, eye: Eye) {
        self.eye = eye;
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
        let r = if serial == 0 { Some(&self.shell) } else { self.children.get(&serial).map(|c| &c.renderer) };
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
        let eye = self.eye;
        // With Screens in effect the shell's 3D lines are drawn over the
        // panels, not in its own screen (SPEC 12.3).
        let layers = scene_layers(system, eye);
        show(&mut self.shell, queue, shell, layout(0), eye, layers.is_none());
        // Every screen's phosphor light of this frame, in one submission
        // (none if no screen is in the phosphor style).
        let mut encoder = None;
        let mut light = |r: &mut Renderer, viewport: (u32, u32)| {
            if r.phosphor() {
                let e = encoder.get_or_insert_with(|| {
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("phosphor") })
                });
                r.accumulate_into(device, queue, e, viewport);
            }
        };
        if ran.contains(&0) {
            light(&mut self.shell, viewport);
        }

        // The children's pictures, in the shell's units, to pixels.
        let shell_rect = self.shell.screen_rect(viewport);
        let mut keep = Vec::new();
        let (no_trails, force, animate) = (self.no_trails, self.force_phosphor, self.animate_page);
        let fresh_renderer = |shell_r: &Renderer, format, width: u32| {
            let mut r = Renderer::new(shell_r.shaders(), format, shell_r.glyphs_for(width));
            r.set_no_trails(no_trails);
            r.set_force_phosphor(force);
            r.set_page_animation(animate);
            r
        };
        for (h, p) in system.pictures() {
            let Some(child) = system.child(h) else { continue };
            let Some(machine) = &child.machine else { continue };
            let rect = picture_rect(shell_rect, p);
            // A panel's renderer draws into its texture's format.
            if self.children.get(&child.serial).is_some_and(|c| c.panel.is_some()) {
                self.children.remove(&child.serial);
            }
            let fresh = !self.children.contains_key(&child.serial);
            let (format, shell_r) = (self.format, &self.shell);
            let c = self.children.entry(child.serial).or_insert_with(|| {
                let width = ((rect.2 / terse_core::COLS as f32).floor() as u32).max(2);
                let mut r = fresh_renderer(shell_r, format, width);
                r.set_clears(false);
                Shown { handle: h, renderer: r, panel: None, shown: (0, 0) }
            });
            let r = &mut c.renderer;
            r.set_rect(device, Some(rect));
            r.update(queue, &displayed_terminal(machine));
            show(r, queue, machine, layout(child.serial), eye, true);
            if fresh || ran.contains(&child.serial) {
                light(r, viewport);
            }
            keep.push(child.serial);
        }
        // The panels: each machine drawn into a texture of the size its
        // panel needs, again when it ran a frame.
        if let Some(layers) = &layers {
            let cap = (shell_rect.2 / terse_core::COLS as f32).floor() as u32;
            let pipeline = self.panel_pipeline.get_or_insert_with(|| PanelPipeline::new(self.shell.shaders(), self.format));
            for p in &layers.panels {
                let Some(child) = system.child(p.handle) else { continue };
                let Some(machine) = &child.machine else { continue };
                if self.children.get(&child.serial).is_some_and(|c| c.panel.is_none()) {
                    self.children.remove(&child.serial);
                }
                let current = self.children.get(&child.serial).and_then(|c| c.panel.as_ref()).map(|(t, _)| t.size.0 / terse_core::COLS as u32);
                let cw = panel_cell_width(panel_width_px(p, eye, shell_rect), cap, current);
                let size = (cw * terse_core::COLS as u32, cw * terse_core::COLS as u32 * 3 / 4);
                let fresh = current.is_none();
                let shell_r = &self.shell;
                let c = self.children.entry(child.serial).or_insert_with(|| {
                    let mut r = fresh_renderer(shell_r, PANEL_FORMAT, cw);
                    r.set_clears(true);
                    Shown { handle: p.handle, renderer: r, panel: Some((pipeline.texture(device, size), true)), shown: (0, SETTLE) }
                });
                let resized = current.is_some_and(|w| w != cw);
                if resized {
                    c.panel = Some((pipeline.texture(device, size), true));
                }
                let r = &mut c.renderer;
                r.set_rect(device, Some((0.0, 0.0, size.0 as f32, size.1 as f32)));
                r.update(queue, &displayed_terminal(machine));
                let shown_layout = layout(child.serial);
                let d = digest(machine, &shown_layout);
                show(r, queue, machine, shown_layout, eye, true);
                // Drawn again only when what it shows changed (or glows).
                if d != c.shown.0 {
                    c.shown = (d, SETTLE);
                }
                let changed = c.shown.1 > 0 || r.phosphor();
                if fresh || resized || (ran.contains(&child.serial) && changed) {
                    c.shown.1 = c.shown.1.saturating_sub(1);
                    light(r, size);
                    if let Some((_, dirty)) = &mut c.panel {
                        *dirty = true;
                    }
                }
                keep.push(child.serial);
            }
        }
        if let Some(e) = encoder {
            queue.submit([e.finish()]);
        }
        // Hidden or gone: freed.
        self.children.retain(|serial, _| keep.contains(serial));
        self.scene = layers;
    }

    /// Redraws every phosphor beam from what is shown now, without
    /// advancing the afterglow (a screenshot's last picture).
    pub fn refresh_beams(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, viewport: (u32, u32)) {
        self.shell.refresh_beam(device, queue, viewport);
        for c in self.children.values_mut() {
            match &mut c.panel {
                Some((t, dirty)) => {
                    c.renderer.refresh_beam(device, queue, t.size);
                    *dirty = true;
                }
                None => c.renderer.refresh_beam(device, queue, viewport),
            }
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
        if let (Some(layers), Some(pipeline)) = (&self.scene, &self.panel_pipeline) {
            // Each panel's machine into its texture, where it changed.
            for c in self.children.values_mut() {
                if let Some((t, dirty)) = &mut c.panel
                    && *dirty
                {
                    c.renderer.render(device, queue, encoder, &t.level0, t.size);
                    pipeline.mipmaps(encoder, t);
                    *dirty = false;
                }
            }
            let rect = self.shell.screen_rect(viewport);
            let textures: Vec<&PanelTexture> = layers
                .panels
                .iter()
                .filter_map(|p| self.children.values().find(|c| c.handle == p.handle).and_then(|c| c.panel.as_ref()).map(|(t, _)| t))
                .collect();
            // Every panel has a texture (made in `update`).
            let panels: Vec<_> = layers.panels.iter().filter(|p| self.children.values().any(|c| c.handle == p.handle && c.panel.is_some())).copied().collect();
            pipeline.upload(queue, &panels, eye_point(self.eye), rect, viewport);
            let lines = self.lines.get_or_insert_with(|| VectorLayer::for_target(self.shell.shaders(), self.format));
            lines.upload(queue, &layers.lines);
            if let Some(scissor) = scissor(rect, viewport) {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("panels and lines"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pipeline.draw(&mut pass, &textures, scissor);
                lines.draw(queue, &mut pass, viewport, rect);
            }
        }
        let mut order: Vec<_> = self.children.values_mut().filter(|c| c.panel.is_none()).collect();
        order.sort_by_key(|c| c.handle);
        for c in order {
            c.renderer.render(device, queue, encoder, target, viewport);
        }
        self.shell.render_overlay(encoder, target, viewport);
    }
}

/// The whole pixels of `rect` inside a `viewport` target, as a scissor.
fn scissor(rect: (f32, f32, f32, f32), viewport: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    let (x, y, w, h) = rect;
    let x0 = (x.max(0.0).floor() as u32).min(viewport.0);
    let y0 = (y.max(0.0).floor() as u32).min(viewport.1);
    let x1 = ((x + w).ceil().max(0.0) as u32).min(viewport.0);
    let y1 = ((y + h).ceil().max(0.0) as u32).min(viewport.1);
    (x1 > x0 && y1 > y0).then(|| (x0, y0, x1 - x0, y1 - y0))
}

/// Uploads a machine's vector list and page to its renderer; its 3D list
/// only with `lines` (the shell's goes over the panels when Screens are in
/// effect).
fn show(r: &mut Renderer, queue: &wgpu::Queue, machine: &Machine, layout: Option<Arc<HostLayout>>, eye: Eye, lines: bool) {
    r.update_vectors(queue, &VectorScene { vector: machine.vector(), vector3: machine.vector3().filter(|_| lines), eye });
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

    fn set_eye(&mut self, eye: Eye) {
        self.screens.set_eye(eye);
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
