//! The windowed host: winit event loop, 60 frames/s of virtual time, input.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use terse_core::{COLS, Event, FrameInput, FrameMachine, MAX_RANDOM_READS_PER_FRAME, ROWS, buttons};
use terse_rv::{AnyMachine, Program};
use terse_net::ContentService;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{ModifiersState, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::gamepad::Gamepads;
use crate::glyphs::SystemGlyphs;
use crate::keymap::{hid_usage, modifier_bits};
use crate::renderer::{GlyphSource, Renderer};
use crate::{FONT_PX, request_device, terminal_or_blank};

const FRAME: Duration = Duration::from_nanos(1_000_000_000 / 60);

pub fn run(image: Program, content: Option<ContentService>) -> Result<ExitCode, String> {
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let title = format!("{} — Terse", image.title_lossy());
    let mut app = App {
        machine: AnyMachine::new(&image),
        content,
        title,
        gfx: None,
        error: None,
        pending: Vec::new(),
        modifiers: ModifiersState::empty(),
        buttons: 0,
        pointer: None,
        pointer_sent: None,
        wheel: (0.0, 0.0),
        gamepads: Gamepads::new(),
        next_frame: Instant::now(),
        random: vec![0; MAX_RANDOM_READS_PER_FRAME],
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    if let Some(e) = app.error {
        return Err(e);
    }
    Ok(ExitCode::from(app.machine.exit_code().unwrap_or(0)))
}

struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
}

impl Gfx {
    fn new(event_loop: &ActiveEventLoop, title: &str) -> Result<Self, String> {
        // Size the window for the grid at the primary monitor's scale.
        let scale = event_loop.primary_monitor().map_or(1.0, |m| m.scale_factor());
        let glyphs = SystemGlyphs::new(FONT_PX * scale as f32)?;
        eprintln!("terse-host: font {}", glyphs.primary_family());

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
            Box::new(event_loop.owned_display_handle()),
        ));
        let (cell_w, cell_h) = glyphs.cell_size();
        let grid = PhysicalSize::new(cell_w * COLS as u32, cell_h * ROWS as u32);
        let attributes = Window::default_attributes().with_title(title).with_inner_size(grid);
        let window = Arc::new(event_loop.create_window(attributes).map_err(|e| e.to_string())?);
        let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
        let options = wgpu::RequestAdapterOptions { compatible_surface: Some(&surface), ..Default::default() };
        let (adapter, device, queue) = pollster::block_on(request_device(options, &instance))?;
        let info = adapter.get_info();
        eprintln!("terse-host: adapter {} ({:?}, {:?})", info.name, info.backend, info.device_type);

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
        let size = Some(window.inner_size()).filter(|s| s.width > 0 && s.height > 0).unwrap_or(grid);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);
        let renderer = Renderer::new(&device, format, Box::new(glyphs));
        Ok(Gfx { window, surface, device, queue, config, renderer })
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width > 0 && size.height > 0 {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    fn viewport(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    fn draw(&mut self) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let viewport = self.viewport();
        self.renderer.render(&self.device, &self.queue, &mut encoder, &view, viewport);
        self.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        self.queue.present(frame);
    }
}

struct App {
    machine: AnyMachine,
    /// Carries out content requests (device 4) off the frame loop.
    content: Option<ContentService>,
    title: String,
    gfx: Option<Gfx>,
    error: Option<String>,
    /// Input events collected since the last frame.
    pending: Vec<Event>,
    modifiers: ModifiersState,
    buttons: u8,
    /// The pointer in screen units, and the position last sent (SPEC 5.1:
    /// movement is sent at most once per frame).
    pointer: Option<(u16, u16)>,
    pointer_sent: Option<(u16, u16)>,
    /// Wheel steps not yet sent (vertical, horizontal; positive is down, right).
    wheel: (f64, f64),
    gamepads: Gamepads,
    next_frame: Instant,
    random: Vec<u8>,
}

impl App {
    /// Runs one machine frame with the input gathered since the last one.
    fn step(&mut self, event_loop: &ActiveEventLoop) {
        let unix_time = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as u32);
        if getrandom::fill(&mut self.random).is_err() {
            self.random.fill(0);
        }
        self.coalesced_input();
        // SPEC 7.3: whatever finished since the last frame completes now.
        let completions = self.content.as_mut().map_or_else(Vec::new, ContentService::take_finished);
        let controllers = self.gamepads.poll();
        let input =
            FrameInput { events: &self.pending, unix_time, random: &self.random, completions: &completions, controllers };
        let result = self.machine.run_frame(&input);
        self.pending.clear();
        if let Some(content) = &mut self.content {
            content.submit(self.machine.take_content_requests());
        }
        if let Some(gfx) = &mut self.gfx {
            gfx.renderer.update(&gfx.queue, &terminal_or_blank(&self.machine));
            gfx.renderer.update_vectors(&gfx.queue, self.machine.vector_list(), self.machine.vector_generation());
            gfx.window.request_redraw();
        }
        if result.exit.is_some() {
            event_loop.exit();
        }
    }

    fn key(&mut self, event: KeyEvent) {
        let pressed = event.state == ElementState::Pressed;
        if let PhysicalKey::Code(code) = event.physical_key
            && let Some(usage) = hid_usage(code)
        {
            let mods = modifier_bits(self.modifiers);
            self.pending.push(if pressed { Event::key_down(usage, mods) } else { Event::key_up(usage, mods) });
        }
        if pressed && let Some(text) = &event.text {
            self.pending.extend(text.chars().filter(|c| !c.is_control()).map(Event::text));
        }
    }

    /// Pointer movement and wheel steps gathered since the last frame,
    /// queued as at most one event each; also the feeds-updated event.
    fn coalesced_input(&mut self) {
        if let Some((x, y)) = self.pointer
            && self.pointer != self.pointer_sent
        {
            self.pending.push(Event::pointer(x, y, self.buttons));
            self.pointer_sent = self.pointer;
        }
        let steps = |acc: &mut f64| {
            let whole = acc.trunc().clamp(-127.0, 127.0);
            *acc -= whole;
            whole as i8
        };
        let (v, h) = (steps(&mut self.wheel.0), steps(&mut self.wheel.1));
        if v != 0 || h != 0 {
            self.pending.push(Event::wheel(v, h));
        }
        // SPEC 5.1, 7.7: new entries were stored since the previous frame.
        if self.content.as_mut().is_some_and(ContentService::feeds_updated) {
            self.pending.push(Event::feeds_updated());
        }
    }

    fn pointer_moved(&mut self, pos: PhysicalPosition<f64>) {
        let Some(gfx) = &self.gfx else { return };
        self.pointer = Some(gfx.renderer.screen_pos(gfx.viewport(), pos.x, pos.y));
    }

    /// winit's deltas are positive when content should move right and down,
    /// that is, when scrolling left and up; SPEC 5.1 steps are the opposite.
    fn wheel(&mut self, delta: MouseScrollDelta) {
        let (x, y) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x as f64, y as f64),
            MouseScrollDelta::PixelDelta(p) => {
                // One step per terminal row of travel.
                let row = self.gfx.as_ref().map_or(16.0, |g| g.renderer.row_height(g.viewport())).max(1.0);
                (p.x / row, p.y / row)
            }
        };
        self.wheel.0 -= y;
        self.wheel.1 -= x;
    }

    fn mouse_button(&mut self, state: ElementState, button: MouseButton) {
        let bit = match button {
            MouseButton::Left => buttons::PRIMARY,
            MouseButton::Right => buttons::SECONDARY,
            MouseButton::Middle => buttons::MIDDLE,
            _ => return,
        };
        match state {
            ElementState::Pressed => self.buttons |= bit,
            ElementState::Released => self.buttons &= !bit,
        }
        // Button changes are sent at once, in order with key events, at the
        // current position.
        if let Some((x, y)) = self.pointer {
            self.pending.push(Event::pointer(x, y, self.buttons));
            self.pointer_sent = self.pointer;
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        match Gfx::new(event_loop, &self.title) {
            Ok(gfx) => self.gfx = Some(gfx),
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
                return;
            }
        }
        self.next_frame = Instant::now();
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gfx) = &mut self.gfx {
                    gfx.resize(size);
                    gfx.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(gfx) = &mut self.gfx {
                    gfx.draw();
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => self.key(event),
            WindowEvent::CursorMoved { position, .. } => self.pointer_moved(position),
            WindowEvent::MouseInput { state, button, .. } => self.mouse_button(state, button),
            WindowEvent::MouseWheel { delta, .. } => self.wheel(delta),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_none() {
            return;
        }
        let now = Instant::now();
        if now >= self.next_frame {
            self.step(event_loop);
            // SPEC 1.3: at most one frame per tick; if the host fell behind,
            // virtual time lags instead of catching up.
            self.next_frame += FRAME;
            if self.next_frame <= now {
                self.next_frame = now + FRAME;
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
    }
}
