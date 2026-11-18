//! The windowed host: winit event loop, 60 frames/s of virtual time, input.

use std::collections::VecDeque;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use terse_core::system::QUEUE_CAPACITY;
use terse_core::{COLS, Event, FrameInput, Image, MAX_RANDOM_PER_FRAME, Machine, ROWS, buttons};
use terse_net::ContentService;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::gamepad::Gamepads;
use crate::glyphs::SystemGlyphs;
use crate::keymap::{hid_usage, modifier_bits};
use crate::page::PageWorker;
use crate::renderer::{GlyphSource, Renderer};
use crate::selftest::SelfTest;
use crate::{FONT_PX, Looks, consent, displayed_terminal, fault_exit, request_device};

const FRAME: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// The most pasted characters held for delivery.
const MAX_PASTE: usize = 1 << 20;

/// The characters of pasted text the program receives: newlines (a CR LF
/// pair or a lone CR counts as one) and no other control characters.
pub fn paste_chars(text: &str) -> impl Iterator<Item = char> + '_ {
    let mut prev = '\0';
    text.chars().filter_map(move |c| {
        let out = match c {
            '\r' => Some('\n'),
            '\n' if prev == '\r' => None,
            '\n' => Some('\n'),
            c if c.is_control() => None,
            c => Some(c),
        };
        prev = c;
        out
    })
}

/// The answer a key press gives the consent prompt: Y yes; N or Esc no;
/// any other key (Enter included) nothing.
fn prompt_answer(event: &KeyEvent) -> Option<bool> {
    match &event.logical_key {
        Key::Named(NamedKey::Escape) => Some(false),
        Key::Character(c) if c.eq_ignore_ascii_case("y") => Some(true),
        Key::Character(c) if c.eq_ignore_ascii_case("n") => Some(false),
        _ => None,
    }
}

/// Runs `image` in a window. With `selftest`, input comes only from its
/// script and the host measures itself (see `selftest.rs`).
pub fn run(
    image: Image,
    content: Option<ContentService>,
    selftest: Option<SelfTest>,
    looks: Looks,
) -> Result<ExitCode, String> {
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let title = format!("{} — Terse", image.title_lossy());
    let program = image.title_lossy();
    // SPEC 11.4: the page is laid out on its own thread.
    let page = image.has_device(7).then(|| PageWorker::start(looks.page_fonts.clone()));
    let mut app = App {
        machine: Machine::new(&image),
        content,
        title,
        program,
        paste: VecDeque::new(),
        swallowed: Vec::new(),
        clipboard: None,
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
        random: fresh_random(),
        selftest,
        refresh_hz: None,
        no_trails: looks.no_trails,
        force_phosphor: looks.force_phosphor,
        font: looks.font,
        page,
        pointer_px: None,
        trace: std::env::var_os("TERSE_PAGE_TRACE").is_some(),
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    if let Some(e) = app.error {
        return Err(e);
    }
    if let Some(t) = &app.selftest {
        t.report(app.refresh_hz);
        return Ok(ExitCode::SUCCESS);
    }
    if app.machine.fault().is_some() {
        return Ok(fault_exit());
    }
    Ok(ExitCode::from(app.machine.exit_code().unwrap_or(0)))
}

/// A buffer of random bytes for the `random` calls of a frame (SPEC 4).
fn fresh_random() -> Vec<u8> {
    let mut random = vec![0; MAX_RANDOM_PER_FRAME];
    if getrandom::fill(&mut random).is_err() {
        random.fill(0);
    }
    random
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
    fn new(event_loop: &ActiveEventLoop, title: &str, font: Option<&str>) -> Result<Self, String> {
        // Size the window for the grid at the primary monitor's scale.
        let scale = event_loop.primary_monitor().map_or(1.0, |m| m.scale_factor());
        let glyphs = SystemGlyphs::new(FONT_PX * scale as f32, font)?;
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

    /// Draws and presents; returns how long acquiring the swapchain image
    /// took and how long the rest (encode, submit, present) took.
    fn draw(&mut self) -> Option<(Duration, Duration)> {
        let start = Instant::now();
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return None;
            }
            _ => return None,
        };
        let acquired = Instant::now();
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let viewport = self.viewport();
        self.renderer.render(&self.device, &self.queue, &mut encoder, &view, viewport);
        self.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        self.queue.present(frame);
        Some((acquired - start, acquired.elapsed()))
    }
}

struct App {
    machine: Machine,
    /// Carries out content requests (device 4) off the frame loop.
    content: Option<ContentService>,
    title: String,
    /// The image's title, as the consent prompt names it.
    program: String,
    /// Pasted text not yet delivered, fed in as the event queue has room.
    paste: VecDeque<char>,
    /// Keys (HID usages) whose presses went to the host (the prompt, a
    /// paste), so their releases do too.
    swallowed: Vec<u8>,
    clipboard: Option<arboard::Clipboard>,
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
    /// Random bytes for the next frame; only those a frame takes are
    /// replaced, so every byte the program sees is fresh.
    random: Vec<u8>,
    selftest: Option<SelfTest>,
    refresh_hz: Option<f64>,
    /// `--no-trails`: the phosphor style without persistence.
    no_trails: bool,
    /// `--force-phosphor`: the phosphor presentation whatever the style.
    force_phosphor: bool,
    /// `--font`: the font for the grid instead of the monospace font.
    font: Option<String>,
    /// The page device's layout worker, if the image has the device.
    page: Option<PageWorker>,
    /// The pointer in window pixels (for the page's anchors).
    pointer_px: Option<(f64, f64)>,
    /// `TERSE_PAGE_TRACE`: print when page layouts are asked for and arrive.
    trace: bool,
}

impl App {
    /// Runs one machine frame with the input gathered since the last one.
    fn step(&mut self, event_loop: &ActiveEventLoop) {
        if self.selftest.as_ref().is_some_and(SelfTest::done) {
            event_loop.exit();
            return;
        }
        if self.machine.stopped().is_some() {
            // A fault stays on screen until the window is closed; the
            // afterglow goes on fading.
            if let Some(gfx) = &mut self.gfx
                && gfx.renderer.phosphor()
            {
                let viewport = gfx.viewport();
                gfx.renderer.accumulate(&gfx.device, &gfx.queue, viewport);
                gfx.window.request_redraw();
            }
            return;
        }
        let unix_time = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as u32);
        self.coalesced_input();
        let mut controllers = self.gamepads.poll();
        if let Some(t) = &self.selftest {
            // The script is the only input (coalesced_input sent nothing).
            for &e in t.script.events(self.machine.frame()) {
                self.pending.push(e);
                // A scripted press over anchored text (SPEC 11.5).
                if e.kind == Event::POINTER {
                    let (x, y, b) = e.pointer_parts();
                    let pressed = b & !self.buttons != 0;
                    self.buttons = b;
                    if let Some(gfx) = self.gfx.as_ref().filter(|_| pressed && self.page.is_some()) {
                        let anchor = gfx.renderer.page_anchor_at_units(x as f32 + 0.5, y as f32 + 0.5);
                        if anchor != 0 {
                            self.pending.push(Event::page_anchor(anchor, b));
                        }
                    }
                }
            }
            controllers = t.script.controllers(self.machine.frame());
        }
        // SPEC 11.5: a layout of the current page finished: its height first.
        if let Some(worker) = &mut self.page {
            match worker.poll(self.machine.page()) {
                Ok(Some(e)) => {
                    if self.trace {
                        let h = e.a as u32 | (e.b as u32) << 8 | (e.c as u32) << 16;
                        let ms = worker.shown.as_ref().map_or(0.0, |l| l.ms);
                        eprintln!("page: frame {}: height {h} queued (layout {ms:.1} ms)", self.machine.frame());
                    }
                    self.pending.insert(0, e);
                }
                Ok(None) => {}
                Err(e) => {
                    self.error = Some(format!("page fonts: {e}"));
                    event_loop.exit();
                    return;
                }
            }
        }
        let start = Instant::now();
        // SPEC 7.3: whatever finished since the last frame completes now.
        let completions = self.content.as_mut().map_or_else(Vec::new, ContentService::take_finished);
        // Pasted text, as much as the queue has room for (with space for
        // the completions' events and a page anchor), the rest later.
        let room = QUEUE_CAPACITY
            .saturating_sub(self.machine.system().queue().len() + self.pending.len() + completions.len() + 1);
        let n = room.min(self.paste.len());
        self.pending.extend(self.paste.drain(..n).map(Event::text));
        let input =
            FrameInput { events: &self.pending, unix_time, random: &self.random, completions: &completions, controllers };
        let result = self.machine.run_frame(&input);
        self.pending.clear();
        if let Some(worker) = &mut self.page {
            let generation = self.machine.page().map(|p| p.generation());
            if let Some(e) = worker.after_frame(self.machine.page()) {
                self.pending.push(e);
            }
            if self.trace && worker.busy(self.machine.page()) {
                eprintln!("page: frame {}: layout pending (generation {generation:?})", self.machine.frame() - 1);
            }
        }
        if getrandom::fill(&mut self.random[..result.random_used]).is_err() {
            self.random[..result.random_used].fill(0);
        }
        if let Some(content) = &mut self.content {
            content.submit(self.machine.take_content_requests());
        }
        let emulate = start.elapsed();
        let start = Instant::now();
        if let Some(gfx) = &mut self.gfx {
            let shown = displayed_terminal(&self.machine);
            match self.content.as_ref().and_then(|c| c.prompt().map(|p| (p, c.prompts_waiting()))) {
                Some((prompt, waiting)) => {
                    gfx.renderer.update(&gfx.queue, &consent::with_prompt(&shown, &self.program, prompt, waiting));
                    gfx.renderer.set_overlay(Some(consent::FIRST_ROW));
                }
                None => {
                    gfx.renderer.update(&gfx.queue, &shown);
                    gfx.renderer.set_overlay(None);
                }
            }
            gfx.renderer.update_vectors(&gfx.queue, self.machine.vector());
            gfx.renderer.update_page(self.machine.page(), self.page.as_ref().and_then(|w| w.shown.clone()));
            // SPEC 8.7: the trail fades once per machine frame.
            let viewport = gfx.viewport();
            gfx.renderer.accumulate(&gfx.device, &gfx.queue, viewport);
            gfx.window.request_redraw();
        }
        if let Some(t) = &mut self.selftest {
            t.log.push(crate::selftest::FrameLog { emulate, upload: start.elapsed(), present: None });
        }
        if let Some(fault) = self.machine.fault() {
            eprintln!("terse-host: fault: {fault}");
            if let Some(gfx) = &self.gfx {
                gfx.window.set_title(&format!("{} — fault: {fault}", self.title));
            }
            if self.selftest.is_some() {
                event_loop.exit();
            }
        } else if result.stopped.is_some() {
            event_loop.exit();
        }
    }

    fn key(&mut self, event: KeyEvent) {
        if self.selftest.is_some() {
            return;
        }
        let pressed = event.state == ElementState::Pressed;
        let usage = match event.physical_key {
            PhysicalKey::Code(code) => hid_usage(code),
            _ => None,
        };
        // SPEC 9.5: while the host asks for consent, the keyboard is the host's.
        if pressed && self.content.as_ref().is_some_and(|c| c.prompt().is_some()) {
            self.swallowed.extend(usage);
            if !event.repeat
                && let Some(allow) = prompt_answer(&event)
            {
                self.content.as_mut().unwrap().answer(allow);
            }
            return;
        }
        // The release of a key whose press the host took.
        if !pressed && let Some(u) = usage {
            if let Some(i) = self.swallowed.iter().position(|&s| s == u) {
                self.swallowed.swap_remove(i);
                return;
            }
        }
        // Paste: Ctrl+V or Shift+Insert gives the clipboard's text as text events.
        let ctrl_v = self.modifiers.control_key()
            && !self.modifiers.alt_key()
            && !self.modifiers.super_key()
            && event.physical_key == PhysicalKey::Code(KeyCode::KeyV);
        let shift_insert = self.modifiers.shift_key() && event.physical_key == PhysicalKey::Code(KeyCode::Insert);
        if pressed && (ctrl_v || shift_insert) {
            self.swallowed.extend(usage);
            self.paste_clipboard();
            return;
        }
        if let Some(usage) = usage {
            let mods = modifier_bits(self.modifiers);
            self.pending.push(if pressed { Event::key_down(usage, mods) } else { Event::key_up(usage, mods) });
        }
        if pressed && let Some(text) = &event.text {
            self.pending.extend(text.chars().filter(|c| !c.is_control()).map(Event::text));
        }
    }

    /// Queues the clipboard's text for delivery as text events (SPEC 5):
    /// newlines kept, other control characters dropped.
    fn paste_clipboard(&mut self) {
        if self.clipboard.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => self.clipboard = Some(c),
                Err(e) => return eprintln!("terse-host: clipboard: {e}"),
            }
        }
        let text = match self.clipboard.as_mut().unwrap().get_text() {
            Ok(t) => t,
            Err(e) => return eprintln!("terse-host: clipboard: {e}"),
        };
        let room = MAX_PASTE.saturating_sub(self.paste.len());
        self.paste.extend(paste_chars(&text).take(room));
    }

    /// Pointer movement and wheel steps gathered since the last frame,
    /// queued as at most one event each; also the feeds-updated event.
    fn coalesced_input(&mut self) {
        if self.selftest.is_some() {
            return;
        }
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
        self.pointer_px = Some((pos.x, pos.y));
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
        if self.selftest.is_some() {
            return;
        }
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
        // SPEC 11.5: a press over anchored text, as the page is shown now.
        if state == ElementState::Pressed
            && self.page.is_some()
            && let (Some(gfx), Some((x, y))) = (&self.gfx, self.pointer_px)
        {
            let anchor = gfx.renderer.page_anchor_at(gfx.viewport(), x, y);
            if anchor != 0 {
                self.pending.push(Event::page_anchor(anchor, self.buttons));
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        match Gfx::new(event_loop, &self.title, self.font.as_deref()) {
            Ok(mut gfx) => {
                gfx.renderer.set_no_trails(self.no_trails);
                gfx.renderer.set_force_phosphor(self.force_phosphor);
                self.refresh_hz = gfx
                    .window
                    .current_monitor()
                    .and_then(|m| m.refresh_rate_millihertz())
                    .map(|mhz| mhz as f64 / 1000.0);
                self.gfx = Some(gfx);
            }
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
                if let Some(gfx) = &mut self.gfx
                    && let Some((acquire, render)) = gfx.draw()
                {
                    if self.trace {
                        for line in gfx.renderer.take_page_figure_log() {
                            eprintln!("page: frame {}: {line}", self.machine.frame());
                        }
                    }
                    if let Some(t) = &mut self.selftest {
                        t.presented(acquire, render, Instant::now());
                    }
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

#[cfg(test)]
mod tests {
    #[test]
    fn pasted_text_keeps_newlines_only() {
        let got: String = super::paste_chars("a\tb\r\nc\rd\ne\u{1b}[0m\u{7f}é日").collect();
        assert_eq!(got, "ab\nc\nd\ne[0mé日");
    }
}
