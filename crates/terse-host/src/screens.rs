//! The screens of a system's machines drawn together (SPEC §machines.screens): the
//! shell's screen over the whole window, then each machine's picture in its
//! rectangle, in increasing handle order, each a complete screen with its
//! own layers, glyphs rasterised for its size and its own phosphor tube;
//! then the host's consent prompt over everything. A single machine is the
//! shell's screen alone, drawn exactly as before.
//!
//! A machine whose 3D list has Surfaces in effect (SPEC §vector3d.presentation) is drawn as
//! its glass (its renderer, without its 3D list), the Surfaces' sources
//! blacked out on it, its Surfaces (its 2D layers drawn again by a second
//! renderer into a texture of the size they need, when they changed, and
//! mapped in perspective as panels are) and its 3D lines over them; the
//! same wherever its screen is shown: on its own, flat in a tile, or into
//! its panel's texture.

use std::collections::HashMap;
use std::sync::Arc;

use terse_core::{Machine, System};
use terse_net::Prompt;

use crate::display::{
    Display, Eye, SceneLayers, ShownPrompt, VectorScene, cell_width_for, eye_point, panel_cell_width, panel_width_px, picture_rect,
    scene_layers, surface_layers, surface_size, system_rect,
};
use crate::page::HostLayout;
use crate::panels::{PANEL_FORMAT, PanelPipeline, PanelQuads, PanelTexture, Quad};
use crate::renderer::{Renderer, Shaders};
use crate::vectors::VectorLayer;
use crate::{consent, shown_terminal};
use crate::display::Caps;
use terse_core::system_ink::Share;

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
    /// Its own Surfaces, if its list has any in effect.
    surfaced: Option<Surfaced>,
}

/// A machine's own Surfaces as drawn (SPEC §vector3d.presentation): its 2D layers drawn by a
/// renderer of their own into a texture of the size the Surfaces need
/// (again when they changed), and what goes over its glass: the sources
/// blacked out, the Surfaces, its 3D lines.
struct Surfaced {
    renderer: Renderer,
    texture: PanelTexture,
    /// The texture must be drawn again.
    dirty: bool,
    /// As `Shown::shown`.
    shown: (u64, u8),
    layers: SceneLayers,
    /// The format of the target it is drawn over.
    format: wgpu::TextureFormat,
    pipeline: Arc<PanelPipeline>,
    quads: PanelQuads,
    lines: VectorLayer,
    /// The lines by the beam (the glass in the phosphor style's MAME model).
    beam: crate::beamlines::BeamLines,
}

impl Surfaced {
    /// Draws the 2D layers into the texture if they changed.
    fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder) {
        if self.dirty {
            self.renderer.render(device, queue, encoder, &self.texture.level0, self.texture.size);
            self.pipeline.mipmaps(encoder, &self.texture);
            self.dirty = false;
        }
    }

    /// Draws over the glass, a screen drawn into `rect` of a `viewport`
    /// target: the sources black, the Surfaces far to near, the lines.
    #[allow(clippy::too_many_arguments)]
    /// `beam`: the glass's look when its lines are drawn by the beam.
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        shaders: &Shaders,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        rect: (f32, f32, f32, f32),
        viewport: (u32, u32),
        eye: Eye,
        beam: Option<crate::mame::Params>,
    ) {
        let mut quads: Vec<Quad> = self.layers.rects.iter().map(|&r| Quad::black(r)).collect();
        quads.extend(self.layers.panels.iter().map(Quad::panel));
        self.pipeline.upload(queue, &mut self.quads, &quads, eye_point(eye), rect, viewport);
        let Some(scissor) = scissor(rect, viewport) else { return };
        let textures = vec![&self.texture; quads.len()];
        {
            let mut pass = load_pass(encoder, target, "surfaces and lines");
            self.pipeline.draw(&mut pass, &self.quads, &textures, scissor);
            if beam.is_none() {
                self.lines.draw(queue, &mut pass, viewport, rect);
            }
        }
        if let Some(params) = beam {
            self.beam.draw(shaders, queue, encoder, target, self.format, &self.layers.lines, rect, viewport, params);
        }
    }
}

/// A render pass over what `target` holds.
fn load_pass<'e>(encoder: &'e mut wgpu::CommandEncoder, target: &wgpu::TextureView, label: &str) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
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
    })
}

/// Frames a panel is drawn again after its machine's screen changed.
const SETTLE: u8 = 30;

/// A digest of what a machine's screen shows: its terminal, lists, page
/// and the page's layout, and the eye if it has a 3D list; the same
/// digest, the same picture (but for the phosphor style's afterglow and
/// the page's scroll animation).
fn digest(machine: &Machine, layout: &Option<Arc<HostLayout>>, eye: Eye, share: Option<&Share>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    share.map(|s| (s.plate, s.cap2, s.cap3.to_bits())).hash(&mut h);
    let t = shown_terminal(machine, share);
    t.cell_bytes().hash(&mut h);
    t.palette().hash(&mut h);
    (t.cursor(), t.shows_cursor()).hash(&mut h);
    machine.vector().map(|v| (v.generation(), v.phosphor())).hash(&mut h);
    machine.vector3().map(|v| v.generation()).hash(&mut h);
    machine.vector3().filter(|v| !v.list().is_empty()).map(|_| (eye.0.to_bits(), eye.1.to_bits())).hash(&mut h);
    machine.page().map(|p| (p.generation(), p.scroll(), p.region().x, p.region().y, p.region().w, p.region().h, p.background())).hash(&mut h);
    layout.as_ref().map(|l| Arc::as_ptr(l) as usize).hash(&mut h);
    h.finish()
}

/// The panel pipeline for a target format, made the first time.
fn pipeline_for(pipelines: &mut Vec<(wgpu::TextureFormat, Arc<PanelPipeline>)>, shaders: &Shaders, format: wgpu::TextureFormat) -> Arc<PanelPipeline> {
    if let Some((_, p)) = pipelines.iter().find(|(f, _)| *f == format) {
        return p.clone();
    }
    let p = Arc::new(PanelPipeline::new(shaders, format));
    pipelines.push((format, p.clone()));
    p
}

/// What making a machine's Surfaces needs from the screens.
struct Make<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    shaders: &'a Shaders,
    pipelines: &'a mut Vec<(wgpu::TextureFormat, Arc<PanelPipeline>)>,
    /// A renderer for cells this many pixels wide, configured as the
    /// screens' are.
    renderer: &'a dyn Fn(u32) -> Renderer,
    eye: Eye,
}

/// Brings a machine's Surfaces up to date: `layers` its own scene (none:
/// it has no Surface in effect), `glass` the pixels its screen is drawn
/// into on a target of `format`; `ran`: it ran a frame (its phosphor light
/// advances).
fn update_surfaced(
    slot: &mut Option<Surfaced>,
    layers: Option<SceneLayers>,
    machine: &Machine,
    share: Option<&Share>,
    layout: Option<Arc<HostLayout>>,
    glass: (f32, f32, f32, f32),
    format: wgpu::TextureFormat,
    ran: bool,
    make: &mut Make,
) {
    let Some(layers) = layers else {
        *slot = None;
        return;
    };
    if slot.as_ref().is_some_and(|s| s.format != format) {
        *slot = None;
    }
    let current = slot.as_ref().map(|s| s.texture.size);
    let size = surface_size(&layers.panels, make.eye, glass, current);
    let fresh = slot.is_none();
    let s = slot.get_or_insert_with(|| {
        let pipeline = pipeline_for(make.pipelines, make.shaders, format);
        let mut renderer = (make.renderer)(cell_width_for((0.0, 0.0, size.0 as f32, size.1 as f32)));
        renderer.set_clears(true);
        Surfaced {
            renderer,
            texture: pipeline.texture(make.device, size),
            dirty: true,
            shown: (0, SETTLE),
            layers: layers.clone(),
            format,
            quads: pipeline.quads(make.device),
            lines: VectorLayer::for_target(make.shaders, format),
            beam: crate::beamlines::BeamLines::new(),
            pipeline,
        }
    });
    let resized = s.texture.size != size;
    if resized {
        s.texture = s.pipeline.texture(make.device, size);
    }
    let r = &mut s.renderer;
    r.set_rect(make.device, Some((0.0, 0.0, size.0 as f32, size.1 as f32)));
    r.update(make.queue, &shown_terminal(machine, share));
    let d = digest(machine, &layout, (0.0, 0.0), share);
    show(r, make.queue, machine, layout, make.eye, false, share);
    if d != s.shown.0 {
        s.shown = (d, SETTLE);
    }
    let changed = s.shown.1 > 0 || r.phosphor();
    if fresh || resized || (ran && changed) {
        s.shown.1 = s.shown.1.saturating_sub(1);
        if r.phosphor() && (fresh || ran) {
            r.accumulate(make.device, make.queue, size);
        }
        s.dirty = true;
    }
    s.lines.upload(make.queue, &layers.lines);
    s.layers = layers;
}

pub struct Screens {
    /// The shell's screen (or the single machine's).
    pub shell: Renderer,
    /// The shell's own Surfaces (SPEC §vector3d.presentation), if it has any in effect.
    shell_surfaced: Option<Surfaced>,
    /// The children shown, by serial.
    children: HashMap<u32, Shown>,
    /// The shell's 3D scene when Screens are in effect (SPEC §machines.screens), and
    /// what draws it: the panels' quads and the shell's 3D lines over them.
    scene: Option<SceneLayers>,
    scene_quads: Option<PanelQuads>,
    lines: Option<VectorLayer>,
    /// The shell's scene's lines by the beam (the phosphor style's MAME model).
    beam_lines: crate::beamlines::BeamLines,
    /// The panel pipelines, by the format of their target.
    pipelines: Vec<(wgpu::TextureFormat, Arc<PanelPipeline>)>,
    format: wgpu::TextureFormat,
    no_trails: bool,
    force_phosphor: bool,
    animate_page: bool,
    /// How the phosphor style is drawn (`--tube`, `--vignette`).
    look: crate::phosphor::Look,
    /// The eye for 3D lists, the same for every machine (SPEC §vector3d.space).
    eye: Eye,
    /// The host's notice (R15), drawn like the prompt.
    pub notice: Option<consent::Notice>,
}

impl Screens {
    pub fn new(shell: Renderer, format: wgpu::TextureFormat) -> Self {
        Screens {
            shell,
            shell_surfaced: None,
            children: HashMap::new(),
            scene: None,
            scene_quads: None,
            lines: None,
            beam_lines: crate::beamlines::BeamLines::new(),
            pipelines: Vec::new(),
            format,
            no_trails: false,
            force_phosphor: false,
            animate_page: true,
            look: crate::phosphor::Look::default(),
            eye: (0.0, 0.0),
            notice: None,
        }
    }

    pub fn set_look(&mut self, look: crate::phosphor::Look) {
        self.look = look;
        self.shell.set_look(look);
        let surfaced = self.children.values_mut().filter_map(|c| c.surfaced.as_mut()).chain(self.shell_surfaced.as_mut());
        for s in surfaced {
            s.renderer.set_look(look);
        }
        for c in self.children.values_mut() {
            c.renderer.set_look(look);
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
    /// its page was last shown (SPEC §page.events).
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
        let shown = crate::displayed_terminal(shell);
        match &prompt {
            Some((p, waiting, title)) => {
                self.shell.update(queue, &consent::with_prompt(&shown, title, p, *waiting));
                self.shell.set_overlay(Some(consent::FIRST_ROW));
            }
            None => match &self.notice {
                Some(n) => {
                    self.shell.update(queue, &consent::with_notice(&shown, &n.heading, &n.what, &n.then));
                    self.shell.set_overlay(Some(consent::FIRST_ROW));
                }
                None => {
                    self.shell.update(queue, &shown);
                    self.shell.set_overlay(None);
                }
            },
        }
        let eye = self.eye;
        // With Screens in effect the shell's 3D lines are drawn over the
        // panels, not in its own screen (SPEC §machines.screens); with Surfaces, over
        // them (SPEC §vector3d.presentation). The shell's Surfaces are then among the
        // scene's panels, or drawn over its glass on their own.
        // SPEC §machines.ink: each machine's share of the system's ink.
        let shares = crate::system_shares(system);
        let mut layers = scene_layers(system, eye);
        crate::display::plate_panels(&mut layers, &shares);
        let shell_surfaces = match &layers {
            Some(l) => l.has_surfaces().then(|| l.clone()),
            None => surface_layers(shell, eye, terse_core::vector3::INK_CAP),
        };
        show(&mut self.shell, queue, shell, layout(0), eye, layers.is_none() && shell_surfaces.is_none(), None);
        self.shell.set_excluded(shell_surfaces.as_ref().map_or(&[][..], |l| &l.rects));
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
        let (no_trails, force, animate, look) = (self.no_trails, self.force_phosphor, self.animate_page, self.look);
        let shaders = self.shell.shaders().clone();
        let glyphs = |width: u32| self.shell.glyphs_for(width);
        let fresh_renderer = |format, width: u32| {
            let mut r = Renderer::new(&shaders, format, glyphs(width));
            r.set_no_trails(no_trails);
            r.set_force_phosphor(force);
            r.set_page_animation(animate);
            r.set_look(look);
            r
        };
        let surface_renderer = |width: u32| fresh_renderer(PANEL_FORMAT, width);
        let mut make =
            Make { device, queue, shaders: &shaders, pipelines: &mut self.pipelines, renderer: &surface_renderer, eye };
        update_surfaced(&mut self.shell_surfaced, shell_surfaces, shell, None, layout(0), shell_rect, self.format, ran.contains(&0), &mut make);
        for (h, p) in system.pictures() {
            let share = shares.of(h).copied();
            let Some(child) = system.child(h) else { continue };
            let Some(machine) = &child.machine else { continue };
            let rect = picture_rect(shell_rect, p);
            // A panel's renderer draws into its texture's format.
            if self.children.get(&child.serial).is_some_and(|c| c.panel.is_some()) {
                self.children.remove(&child.serial);
            }
            let fresh = !self.children.contains_key(&child.serial);
            let format = self.format;
            let c = self.children.entry(child.serial).or_insert_with(|| {
                let width = ((rect.2 / terse_core::COLS as f32).floor() as u32).max(2);
                let mut r = fresh_renderer(format, width);
                r.set_clears(false);
                Shown { handle: h, renderer: r, panel: None, shown: (0, 0), surfaced: None }
            });
            let own = surface_layers(machine, eye, Caps::of(share.as_ref()).ink3).filter(|_| !share.is_some_and(|s| s.plate));
            let r = &mut c.renderer;
            r.set_rect(device, Some(rect));
            r.update(queue, &shown_terminal(machine, share.as_ref()));
            show(r, queue, machine, layout(child.serial), eye, own.is_none(), share.as_ref());
            r.set_excluded(own.as_ref().map_or(&[][..], |l| &l.rects));
            if fresh || ran.contains(&child.serial) {
                light(r, viewport);
            }
            let ran_now = fresh || ran.contains(&child.serial);
            update_surfaced(&mut c.surfaced, own, machine, share.as_ref(), layout(child.serial), rect, format, ran_now, &mut make);
            keep.push(child.serial);
        }
        // The panels: each machine drawn into a texture of the size its
        // panel needs, again when it ran a frame.
        if let Some(layers) = &layers {
            let cap = (shell_rect.2 / terse_core::COLS as f32).floor() as u32;
            for p in layers.panels.iter().filter(|p| !p.is_surface()) {
                let Some(child) = system.child(p.handle) else { continue };
                let share = shares.of(p.handle).copied();
                if share.is_some_and(|s| s.plate) {
                    continue; // drawn as a plate: no texture to draw
                }
                let Some(machine) = &child.machine else { continue };
                if self.children.get(&child.serial).is_some_and(|c| c.panel.is_none()) {
                    self.children.remove(&child.serial);
                }
                let current = self.children.get(&child.serial).and_then(|c| c.panel.as_ref()).map(|(t, _)| t.size.0 / terse_core::COLS as u32);
                let cw = panel_cell_width(panel_width_px(p, eye, shell_rect), cap, current);
                let size = (cw * terse_core::COLS as u32, cw * terse_core::COLS as u32 * 3 / 4);
                let fresh = current.is_none();
                let pipeline = pipeline_for(make.pipelines, &shaders, self.format);
                let c = self.children.entry(child.serial).or_insert_with(|| {
                    let mut r = fresh_renderer(PANEL_FORMAT, cw);
                    r.set_clears(true);
                    Shown { handle: p.handle, renderer: r, panel: Some((pipeline.texture(device, size), true)), shown: (0, SETTLE), surfaced: None }
                });
                let resized = current.is_some_and(|w| w != cw);
                if resized {
                    c.panel = Some((pipeline.texture(device, size), true));
                }
                let own = surface_layers(machine, eye, Caps::of(share.as_ref()).ink3).filter(|_| !share.is_some_and(|s| s.plate));
                let r = &mut c.renderer;
                r.set_rect(device, Some((0.0, 0.0, size.0 as f32, size.1 as f32)));
                r.update(queue, &shown_terminal(machine, share.as_ref()));
                let shown_layout = layout(child.serial);
                let d = digest(machine, &shown_layout, eye, share.as_ref());
                show(r, queue, machine, shown_layout.clone(), eye, own.is_none(), share.as_ref());
                r.set_excluded(own.as_ref().map_or(&[][..], |l| &l.rects));
                // Drawn again only when what it shows changed (or glows).
                if d != c.shown.0 {
                    c.shown = (d, SETTLE);
                }
                let changed = c.shown.1 > 0 || r.phosphor() || c.surfaced.as_ref().is_some_and(|s| s.renderer.phosphor());
                let redraw = fresh || resized || (ran.contains(&child.serial) && changed);
                if redraw {
                    c.shown.1 = c.shown.1.saturating_sub(1);
                    light(r, size);
                    if let Some((_, dirty)) = &mut c.panel {
                        *dirty = true;
                    }
                }
                let ran_now = fresh || ran.contains(&child.serial);
                update_surfaced(&mut c.surfaced, own, machine, share.as_ref(), shown_layout, (0.0, 0.0, size.0 as f32, size.1 as f32), PANEL_FORMAT, ran_now, &mut make);
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
        let surfaced = self.children.values_mut().filter_map(|c| c.surfaced.as_mut()).chain(self.shell_surfaced.as_mut());
        for s in surfaced {
            s.renderer.refresh_beam(device, queue, s.texture.size);
            s.dirty = true;
        }
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
        let eye = self.eye;
        self.shell.render(device, queue, encoder, target, viewport);
        let rect = self.shell.screen_rect(viewport);
        if let Some(s) = &mut self.shell_surfaced {
            s.prepare(device, queue, encoder);
        }
        if let Some(layers) = &self.scene {
            // Each panel's machine into its texture, where it changed, with
            // its own Surfaces.
            for c in self.children.values_mut() {
                if let Some((t, dirty)) = &mut c.panel
                    && *dirty
                {
                    c.renderer.render(device, queue, encoder, &t.level0, t.size);
                    let beam = c.renderer.beam_params();
                    if let Some(s) = &mut c.surfaced {
                        s.prepare(device, queue, encoder);
                        let shaders = c.renderer.shaders().clone();
                        s.draw(&shaders, queue, encoder, &t.level0, (0.0, 0.0, t.size.0 as f32, t.size.1 as f32), t.size, eye, beam);
                    }
                    if let Some(p) = self.pipelines.iter().find(|(f, _)| *f == self.format) {
                        p.1.mipmaps(encoder, t);
                    }
                    *dirty = false;
                }
            }
            let shaders = self.shell.shaders().clone();
            let pipeline = pipeline_for(&mut self.pipelines, &shaders, self.format);
            // The shell's sources black, then the panels with their
            // textures: a Screen its machine's, a Surface the shell's 2D
            // layers.
            let texture = |p: &terse_core::vector3::Panel| -> Option<&PanelTexture> {
                if p.is_surface() {
                    self.shell_surfaced.as_ref().map(|s| &s.texture)
                } else {
                    self.children.values().find(|c| c.handle == p.handle).and_then(|c| c.panel.as_ref()).map(|(t, _)| t)
                }
            };
            // A panel seen from behind (or a plate, SPEC §machines.ink) samples
            // no texture: any one is bound for it.
            let any = layers.panels.iter().find_map(|p| texture(p));
            let drawn: Vec<(Quad, &PanelTexture)> =
                layers.panels.iter().filter_map(|p| Some((Quad::panel(p), texture(p).or(any.filter(|_| !p.front))?))).collect();
            let mut quads: Vec<(Quad, &PanelTexture)> = match drawn.first() {
                Some(&(_, any)) => layers.rects.iter().map(|&r| (Quad::black(r), any)).collect(),
                None => Vec::new(),
            };
            quads.extend(drawn);
            let (shapes, textures): (Vec<Quad>, Vec<&PanelTexture>) = quads.into_iter().unzip();
            let scene_quads = self.scene_quads.get_or_insert_with(|| pipeline.quads(device));
            pipeline.upload(queue, scene_quads, &shapes, eye_point(eye), rect, viewport);
            let lines = self.lines.get_or_insert_with(|| VectorLayer::for_target(&shaders, self.format));
            lines.upload(queue, &layers.lines);
            let beam = self.shell.beam_params();
            if let Some(scissor) = scissor(rect, viewport) {
                let mut pass = load_pass(encoder, target, "panels and lines");
                pipeline.draw(&mut pass, scene_quads, &textures, scissor);
                if beam.is_none() {
                    lines.draw(queue, &mut pass, viewport, rect);
                }
            }
            if let Some(params) = beam {
                self.beam_lines.draw(&shaders, queue, encoder, target, self.format, &layers.lines, rect, viewport, params);
            }
        } else if let Some(s) = &mut self.shell_surfaced {
            let shaders = self.shell.shaders().clone();
            s.draw(&shaders, queue, encoder, target, rect, viewport, eye, self.shell.beam_params());
        }
        let mut order: Vec<_> = self.children.values_mut().filter(|c| c.panel.is_none()).collect();
        order.sort_by_key(|c| c.handle);
        for c in order {
            c.renderer.render(device, queue, encoder, target, viewport);
            if let Some(s) = &mut c.surfaced {
                s.prepare(device, queue, encoder);
                let rect = c.renderer.screen_rect(viewport);
                let shaders = c.renderer.shaders().clone();
                s.draw(&shaders, queue, encoder, target, rect, viewport, eye, c.renderer.beam_params());
            }
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
/// effect, any machine's over its Surfaces).
/// `share`: a machine's share of the system's ink (SPEC §machines.ink): its
/// lists' caps, or a plate, nothing of its screen drawn.
fn show(r: &mut Renderer, queue: &wgpu::Queue, machine: &Machine, layout: Option<Arc<HostLayout>>, eye: Eye, lines: bool, share: Option<&Share>) {
    let caps = Caps::of(share);
    if share.is_some_and(|s| s.plate) {
        r.update_vectors(queue, &VectorScene { vector: None, vector3: None, eye, caps });
        r.update_page(None, None);
        return;
    }
    r.update_vectors(queue, &VectorScene { vector: machine.vector(), vector3: machine.vector3().filter(|_| lines), eye, caps });
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

    fn set_look(&mut self, look: crate::phosphor::Look) {
        self.screens.set_look(look);
    }

    fn set_notice(&mut self, notice: Option<consent::Notice>) {
        self.screens.notice = notice;
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

    fn screen_rect(&self, viewport: (u32, u32)) -> (f32, f32, f32, f32) {
        self.screens.shell.screen_rect(viewport)
    }

    fn take_page_figure_log(&mut self) -> Vec<String> {
        self.screens.shell.take_page_figure_log()
    }

    fn page_shown_generation(&self) -> Option<u64> {
        self.screens.shell.page_shown_generation()
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

/// Copies a presented texture out into a buffer to read (`--drive`).
#[cfg(feature = "window")]
pub fn copy_out(device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture, (width, height): (u32, u32)) -> wgpu::Buffer {
    let padded = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("presented picture"),
        size: (padded * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: None } },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    buffer
}

/// The picture [`copy_out`] copied, as RGBA rows, from a texture of `format`.
#[cfg(feature = "window")]
pub fn read_out(device: &wgpu::Device, buffer: wgpu::Buffer, (width, height): (u32, u32), format: wgpu::TextureFormat) -> Result<crate::drive::Raw, String> {
    let padded = ((width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) as usize;
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| e.to_string())?;
    let data = slice.get_mapped_range().map_err(|e| e.to_string())?;
    let bgra = matches!(format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
    let _ = height;
    Ok(crate::drive::Raw::Rows { data: data.to_vec(), stride: padded, bgra })
}
