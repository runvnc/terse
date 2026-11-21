//! The windowed host: winit event loop, 60 frames/s of virtual time, input.
//! It runs a [`System`]: the shell and the machines it starts (SPEC §machines), or
//! a single machine.

use std::collections::{HashMap, VecDeque};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use terse_core::system::{CONTROLLERS, QUEUE_CAPACITY};
use terse_core::{
    COLS, Completion, Controller, Driver, Event, FrameInput, FrameResult, MAX_RANDOM_PER_FRAME, Machine,
    ROWS, System, Who, buttons,
};
use terse_net::ContentService;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::display::{Display, GlyphSource, RendererChoice};
use crate::gamepad::Gamepads;
use crate::glyphs::TermGlyphs;
use crate::keymap::{HOME_KEY, MAX_PASTE, hid_usage, modifier_bits, paste_chars, prompt_answer, swallow};
use crate::page::{PageThread, PageWorker};
use crate::selftest::SelfTest;
use crate::soft::SoftScreens;
use crate::soft::base::IRect;
use crate::{FONT_PX, Looks, Program, fault_exit};

const FRAME: Duration = Duration::from_nanos(1_000_000_000 / 60);


/// Runs `program` in a window. With `selftest`, input comes only from its
/// script and the host measures itself (see `selftest.rs`).
pub fn run(
    program: Program,
    content: Option<ContentService>,
    selftest: Option<SelfTest>,
    looks: Looks,
) -> Result<ExitCode, String> {
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let title = if program.shell { "Terse".to_string() } else { format!("{} — Terse", program.image.title_lossy()) };
    let mut system = System::new(&program.image, program.shell, program.builtins);
    system.set_shell_identity(program.identity);
    terse_net::snapshots::load_table(&mut system, &program.data);
    let looks_eye = looks.eye;
    // A self-test never makes a sound (it runs unattended, often in a
    // headless compositor); nor does a host told TERSE_AUDIO=off.
    #[cfg(feature = "audio")]
    let audio = if selftest.is_some() || std::env::var_os("TERSE_AUDIO").is_some_and(|v| v == "off") {
        crate::audio::Audio::disabled()
    } else {
        crate::audio::Audio::default()
    };
    let mut app = App {
        system,
        io: HashMap::new(),
        content,
        fetching: Vec::new(),
        page_thread: None,
        looks,
        title,
        shell_title: program.image.title_lossy(),
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
        pointer_px: None,
        eye: looks_eye,
        wheel: (0.0, 0.0),
        gamepads: Gamepads::new(),
        next_frame: Instant::now(),
        occluded: false,
        selftest,
        refresh_hz: None,
        trace: std::env::var_os("TERSE_PAGE_TRACE").is_some(),
        fault_shown: false,
        #[cfg(feature = "audio")]
        audio,
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    if let Some(e) = app.error {
        return Err(e);
    }
    if let Some(t) = &app.selftest {
        t.report(app.refresh_hz);
        return Ok(ExitCode::SUCCESS);
    }
    let shell = app.system.shell();
    if shell.fault().is_some() {
        return Ok(fault_exit());
    }
    Ok(ExitCode::from(shell.exit_code().unwrap_or(0)))
}

/// A buffer of random bytes for the `random` calls of a frame (SPEC §system).
fn fresh_random() -> Vec<u8> {
    let mut random = vec![0; MAX_RANDOM_PER_FRAME];
    if getrandom::fill(&mut random).is_err() {
        random.fill(0);
    }
    random
}

/// How the window is drawn: by the GPU renderer into a wgpu surface, or by
/// the software renderer into a softbuffer surface (a shared-memory buffer
/// the compositor shows; only the damaged rectangles are copied when the
/// buffer it hands back still holds a recent frame).
enum Presenter {
    #[cfg(feature = "gpu")]
    Gpu { surface: wgpu::Surface<'static>, config: wgpu::SurfaceConfiguration, display: crate::screens::GpuDisplay },
    Soft {
        surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
        display: SoftScreens,
        size: (u32, u32),
        /// The damage of the last frames presented, newest first.
        history: VecDeque<Vec<IRect>>,
    },
}

struct Gfx {
    window: Arc<Window>,
    presenter: Presenter,
}

impl Gfx {
    fn new(event_loop: &ActiveEventLoop, title: &str, looks: &Looks, test: bool) -> Result<Self, String> {
        // Size the window for the grid at the primary monitor's scale.
        let scale = event_loop.primary_monitor().map_or(1.0, |m| m.scale_factor());
        let glyphs = TermGlyphs::new(FONT_PX * scale as f32, &looks.term_fonts)?;
        eprintln!("terse-host: font {}", glyphs.primary_family());

        let (cell_w, cell_h) = glyphs.cell_size();
        let grid = PhysicalSize::new(cell_w * COLS as u32, cell_h * ROWS as u32);
        let inner = looks.window.map_or(grid, |(w, h)| PhysicalSize::new(w, h));
        let attributes = Window::default_attributes().with_title(title).with_inner_size(inner);
        // The window's class (Wayland app ID): a self-test's differs, so a window manager
        // rule can keep test windows out of the way.
        #[cfg(all(unix, not(target_os = "macos")))]
        let attributes = {
            use winit::platform::wayland::WindowAttributesExtWayland;
            let class = if test { "terse-host-test" } else { "terse-host" };
            WindowAttributesExtWayland::with_name(attributes, class, class)
        };
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        let _ = test;
        let window = Arc::new(event_loop.create_window(attributes).map_err(|e| e.to_string())?);
        let size = Some(window.inner_size()).filter(|s| s.width > 0 && s.height > 0).unwrap_or(inner);
        let size = (size.width, size.height);

        if looks.renderer != RendererChoice::Software {
            #[cfg(feature = "gpu")]
            {
                let auto = looks.renderer == RendererChoice::Auto;
                match Self::gpu(event_loop, &window, glyphs.clone(), size, auto) {
                    Ok(presenter) => return Ok(Self::with(window, presenter, looks)),
                    Err(e) if auto => eprintln!("terse-host: {e}; using the software renderer"),
                    Err(e) => return Err(e),
                }
            }
            #[cfg(not(feature = "gpu"))]
            if looks.renderer == RendererChoice::Gpu {
                return Err("this terse-host was built without the GPU renderer (feature \"gpu\")".into());
            }
        }
        // The software renderer, presented with softbuffer. (Not yet tried
        // in a live window: see README, Software renderer.)
        let context = softbuffer::Context::new(window.clone()).map_err(|e| format!("softbuffer: {e}"))?;
        let surface = softbuffer::Surface::new(&context, window.clone()).map_err(|e| format!("softbuffer: {e}"))?;
        let display = SoftScreens::new(Box::new(glyphs), looks.threads);
        eprintln!("terse-host: renderer {}", display.name());
        let mut gfx = Self::with(window, Presenter::Soft { surface, display, size: (0, 0), history: VecDeque::new() }, looks);
        gfx.resize(PhysicalSize::new(size.0, size.1));
        Ok(gfx)
    }

    fn with(window: Arc<Window>, mut presenter: Presenter, looks: &Looks) -> Self {
        let d: &mut dyn Display = match &mut presenter {
            #[cfg(feature = "gpu")]
            Presenter::Gpu { display, .. } => display,
            Presenter::Soft { display, .. } => display,
        };
        d.configure(looks.no_trails, looks.force_phosphor, true);
        d.set_look(looks.look);
        Gfx { window, presenter }
    }

    /// The GPU renderer on a wgpu surface; with `hardware_only`, an error if
    /// the only adapter is a software one.
    #[cfg(feature = "gpu")]
    fn gpu(
        event_loop: &ActiveEventLoop,
        window: &Arc<Window>,
        glyphs: TermGlyphs,
        size: (u32, u32),
        hardware_only: bool,
    ) -> Result<Presenter, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
            Box::new(event_loop.owned_display_handle()),
        ));
        let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
        let options = wgpu::RequestAdapterOptions { compatible_surface: Some(&surface), ..Default::default() };
        let (adapter, device, queue) = pollster::block_on(crate::request_device(options, &instance))?;
        let info = adapter.get_info();
        if hardware_only && info.device_type == wgpu::DeviceType::Cpu {
            return Err(format!("no graphics adapter but a software one ({})", info.name));
        }
        eprintln!("terse-host: adapter {} ({:?}, {:?})", info.name, info.backend, info.device_type);

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
        // Never wait for the display inside a present: a compositor stops
        // asking a hidden window (another workspace) for frames, and a
        // vsync-blocking present then stalls the whole host. The host paces
        // itself at 60 Hz; Mailbox shows the latest frame without tearing.
        // TERSE_VSYNC=1 asks for the blocking mode.
        let vsync = std::env::var_os("TERSE_VSYNC").is_some_and(|v| v == "1");
        let present_mode = if !vsync && caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::AutoVsync
        };
        eprintln!("terse-host: present mode {present_mode:?}");
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.0,
            height: size.1,
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);
        let renderer = crate::renderer::Renderer::new(&crate::renderer::Shaders::new(&device), format, Box::new(glyphs));
        let screens = crate::screens::Screens::new(renderer, format);
        let display = crate::screens::GpuDisplay::new(device, queue, screens, format, info.name);
        Ok(Presenter::Gpu { surface, config, display })
    }

    fn display(&self) -> &dyn Display {
        match &self.presenter {
            #[cfg(feature = "gpu")]
            Presenter::Gpu { display, .. } => display,
            Presenter::Soft { display, .. } => display,
        }
    }

    fn display_mut(&mut self) -> &mut dyn Display {
        match &mut self.presenter {
            #[cfg(feature = "gpu")]
            Presenter::Gpu { display, .. } => display,
            Presenter::Soft { display, .. } => display,
        }
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        match &mut self.presenter {
            #[cfg(feature = "gpu")]
            Presenter::Gpu { surface, config, display } => {
                config.width = size.width;
                config.height = size.height;
                surface.configure(&display.device, config);
            }
            Presenter::Soft { surface, size: current, history, .. } => {
                let (Some(w), Some(h)) = (std::num::NonZeroU32::new(size.width), std::num::NonZeroU32::new(size.height)) else {
                    return;
                };
                match surface.resize(w, h) {
                    Ok(()) => *current = (size.width, size.height),
                    Err(e) => eprintln!("terse-host: softbuffer: {e}"),
                }
                history.clear();
            }
        }
    }

    fn viewport(&self) -> (u32, u32) {
        match &self.presenter {
            #[cfg(feature = "gpu")]
            Presenter::Gpu { config, .. } => (config.width, config.height),
            Presenter::Soft { size, .. } => *size,
        }
    }

    /// Draws and presents; returns how long acquiring the swapchain image
    /// (or buffer) took and how long the rest (draw, present) took.
    fn draw(&mut self) -> Option<(Duration, Duration)> {
        match &mut self.presenter {
            #[cfg(feature = "gpu")]
            Presenter::Gpu { surface, config, display } => {
                let viewport = (config.width, config.height);
                let start = Instant::now();
                let frame = match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
                    wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                        surface.configure(&display.device, config);
                        return None;
                    }
                    _ => return None,
                };
                let acquired = Instant::now();
                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder = display.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                display.screens.render(&display.device, &display.queue, &mut encoder, &view, viewport);
                display.queue.submit([encoder.finish()]);
                self.window.pre_present_notify();
                display.queue.present(frame);
                Some((acquired - start, acquired.elapsed()))
            }
            Presenter::Soft { surface, display, size, history } => {
                if size.0 == 0 || size.1 == 0 {
                    return None;
                }
                let start = Instant::now();
                display.render(*size);
                let damage = display.damage().to_vec();
                let frame = display.frame();
                let drawn = Instant::now();
                let mut buffer = match surface.buffer_mut() {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("terse-host: softbuffer: {e}");
                        return None;
                    }
                };
                let acquired = Instant::now();
                history.push_front(damage);
                history.truncate(4);
                // The buffer holds the frame of `age` presents ago: copy
                // what changed since, or everything.
                let age = buffer.age() as usize;
                let stride = size.0 as usize;
                let rects: Vec<IRect> = if age >= 1 && age <= history.len() {
                    history.iter().take(age).flatten().copied().collect()
                } else {
                    vec![IRect::of_size(*size)]
                };
                if buffer.len() == frame.data.len() {
                    for r in &rects {
                        for y in r.y0..r.y1 {
                            let at = y as usize * stride;
                            buffer[at + r.x0 as usize..at + r.x1 as usize].copy_from_slice(&frame.data[at + r.x0 as usize..at + r.x1 as usize]);
                        }
                    }
                } else {
                    let n = buffer.len().min(frame.data.len());
                    buffer[..n].copy_from_slice(&frame.data[..n]);
                }
                let damage: Vec<softbuffer::Rect> = rects
                    .iter()
                    .filter_map(|r| {
                        Some(softbuffer::Rect {
                            x: r.x0 as u32,
                            y: r.y0 as u32,
                            width: std::num::NonZeroU32::new(r.width() as u32)?,
                            height: std::num::NonZeroU32::new(r.height() as u32)?,
                        })
                    })
                    .collect();
                self.window.pre_present_notify();
                let presented = if damage.is_empty() { buffer.present() } else { buffer.present_with_damage(&damage) };
                if let Err(e) = presented {
                    eprintln!("terse-host: softbuffer: {e}");
                }
                Some((acquired - drawn, (drawn - start) + acquired.elapsed()))
            }
        }
    }
}

/// What the host keeps for one machine of the system.
struct MachineIo {
    /// The page device's layouts, if the image has the device.
    page: Option<PageWorker>,
    /// Random bytes for the next frame; only those a frame takes are
    /// replaced, so every byte the program sees is fresh.
    random: Vec<u8>,
    /// Content requests finished since its last frame.
    completions: Vec<Completion>,
    /// Events for its next frame: a page height reported after a frame.
    carry: Vec<Event>,
    /// New feed entries since its last frame.
    feeds: bool,
    /// Buttons of its last pointer event (to tell presses, for anchors).
    buttons: u8,
}

/// The host's part of a system frame (the system's [`Driver`]): the
/// machines' input besides the routed events, and their requests.
struct FrameDriver<'a> {
    io: &'a mut HashMap<u32, MachineIo>,
    content: &'a mut Option<ContentService>,
    fetching: &'a mut Vec<u32>,
    screens: Option<&'a dyn Display>,
    page_thread: &'a mut Option<PageThread>,
    looks: &'a Looks,
    unix_time: u32,
    /// The machines that ran a frame (serials).
    ran: Vec<u32>,
    trace: bool,
}

impl Driver for FrameDriver<'_> {
    fn frame(&mut self, who: Who, machine: &mut Machine, events: Vec<Event>, controllers: [Controller; CONTROLLERS]) -> FrameResult {
        let (page_thread, looks) = (&mut *self.page_thread, self.looks);
        let io = self.io.entry(who.serial).or_insert_with(|| MachineIo {
            // SPEC §page.layout: pages are laid out on a thread of their own.
            page: machine
                .page()
                .is_some()
                .then(|| page_thread.get_or_insert_with(|| PageThread::start(looks.page_fonts.clone())).worker()),
            random: fresh_random(),
            completions: Vec::new(),
            carry: Vec::new(),
            feeds: false,
            buttons: 0,
        });
        let mut input = std::mem::take(&mut io.carry);
        // SPEC §page.events: a layout of the current page finished: its height first.
        if let Some(worker) = &mut io.page
            && let Ok(page_events) = worker.poll(machine.page())
            && !page_events.is_empty()
        {
            if self.trace {
                eprintln!("page: machine {}: frame {}: {} page events queued", who.handle, machine.frame(), page_events.len());
            }
            input.splice(0..0, page_events);
        }
        for e in events {
            input.push(e);
            // A press over anchored text, as the page is shown now.
            if e.kind == Event::POINTER && machine.page().is_some() {
                let (x, y, b) = e.pointer_parts();
                let pressed = b & !io.buttons != 0;
                io.buttons = b;
                let anchor = self.screens.map_or(0, |s| s.anchor(who.serial, x, y));
                if pressed && anchor != 0 {
                    input.push(Event::page_anchor(anchor, b));
                }
            }
        }
        // SPEC §content.events, §content.feeds: new entries were stored since the previous frame.
        if std::mem::take(&mut io.feeds) {
            input.push(Event::feeds_updated());
        }
        let completions = std::mem::take(&mut io.completions);
        let frame =
            FrameInput { events: &input, unix_time: self.unix_time, random: &io.random, completions: &completions, controllers };
        let result = machine.run_frame(&frame);
        if let Some(worker) = &mut io.page
            && let Some(e) = worker.after_frame(machine.page())
        {
            io.carry.push(e);
        }
        if getrandom::fill(&mut io.random[..result.random_used]).is_err() {
            io.random[..result.random_used].fill(0);
        }
        if let Some(content) = self.content.as_mut() {
            content.submit_machine(who.serial, machine);
        }
        self.ran.push(who.serial);
        result
    }

    fn want_image(&mut self, serial: u32, image: [u8; 32]) {
        if let Some(content) = self.content.as_mut() {
            content.fetch(serial, image);
            self.fetching.push(serial);
        }
    }

    fn want_program(&mut self, serial: u32, program: &terse_core::ProgramId, builtins: &[[u8; 32]]) {
        if let Some(content) = self.content.as_mut() {
            content.fetch_program(serial, program, builtins);
            self.fetching.push(serial);
        }
    }

    fn removed(&mut self, serial: u32) {
        self.io.remove(&serial);
        self.fetching.retain(|&s| s != serial);
        if let Some(content) = self.content.as_mut() {
            content.cancel(serial);
        }
    }

    fn save_snapshot(&mut self, shell: &Machine, slot: u8, snapshot: terse_core::Snapshot) -> Option<bool> {
        let Some(content) = self.content.as_mut() else { return Some(false) };
        content.save_snapshot(terse_net::Owner::of(shell), slot, &snapshot);
        None
    }

    fn want_snapshot(&mut self, serial: u32, shell: &Machine, slot: u8) -> Option<Result<(terse_core::Image, terse_core::Snapshot), terse_core::StartFailure>> {
        let Some(content) = self.content.as_mut() else {
            let failure = terse_core::machines::failure::SNAPSHOT;
            return Some(Err(terse_core::StartFailure { reason: failure, image: None, message: "no content device".into() }));
        };
        content.restore_snapshot(serial, terse_net::Owner::of(shell), slot);
        None
    }

    fn delete_snapshot(&mut self, shell: &Machine, slot: u8) {
        if let Some(content) = self.content.as_mut() {
            content.delete_snapshot(terse_net::Owner::of(shell), slot);
        }
    }
}

struct App {
    system: System,
    io: HashMap<u32, MachineIo>,
    /// Carries out content requests (device 4) off the frame loop, for
    /// every machine, and fetches the images the shell starts.
    content: Option<ContentService>,
    /// Machines whose image is being fetched (serials).
    fetching: Vec<u32>,
    page_thread: Option<PageThread>,
    looks: Looks,
    title: String,
    /// The shell's (or the single machine's) image title.
    shell_title: String,
    /// Pasted text not yet delivered, fed in as the event queue has room.
    paste: VecDeque<char>,
    /// Keys (HID usages) whose presses went to the host (the prompt, a
    /// paste, the home key), so their releases do too.
    swallowed: Vec<u8>,
    clipboard: Option<arboard::Clipboard>,
    gfx: Option<Gfx>,
    error: Option<String>,
    /// Input events collected since the last frame.
    pending: Vec<Event>,
    modifiers: ModifiersState,
    buttons: u8,
    /// The pointer in screen units, and the position last sent (SPEC §events:
    /// movement is sent at most once per frame).
    pointer: Option<(u16, u16)>,
    pointer_sent: Option<(u16, u16)>,
    /// The pointer in the window (pixels), while it is in it: the eye of
    /// the 3D display follows it (SPEC §vector3d.space), smoothed.
    pointer_px: Option<(f64, f64)>,
    eye: crate::display::Eye,
    /// Wheel steps not yet sent (vertical, horizontal; positive is down, right).
    wheel: (f64, f64),
    gamepads: Gamepads,
    next_frame: Instant,
    /// The window is hidden (another workspace, minimised): nothing is drawn.
    occluded: bool,
    selftest: Option<SelfTest>,
    refresh_hz: Option<f64>,
    /// `TERSE_PAGE_TRACE`: print when page layouts are asked for and arrive.
    trace: bool,
    fault_shown: bool,
    /// The sound device's output (SPEC §sound.presentation), opened when a machine
    /// first gives samples.
    #[cfg(feature = "audio")]
    audio: crate::audio::Audio,
}

use terse_net::service::image_from;

impl App {
    /// Whether the system is a shell with machines (the home key is the
    /// host's), not a single machine.
    fn is_system(&self) -> bool {
        self.system.is_system()
    }

    /// Runs one system frame with the input gathered since the last one.
    fn step(&mut self, event_loop: &ActiveEventLoop) {
        if self.selftest.as_ref().is_some_and(SelfTest::done) {
            event_loop.exit();
            return;
        }
        if self.system.stopped().is_some() {
            // A fault stays on screen until the window is closed; the
            // afterglow goes on fading.
            if let Some(gfx) = &mut self.gfx
                && gfx.display().shell_phosphor()
            {
                let viewport = gfx.viewport();
                gfx.display_mut().fade_shell(viewport);
                gfx.window.request_redraw();
            }
            return;
        }
        let unix_time = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as u32);
        self.coalesced_input();
        let mut controllers = self.gamepads.poll();
        if let Some(t) = &self.selftest {
            // The script is the only input (coalesced_input sent nothing).
            let frame = self.system.shell().frame();
            self.pending.extend_from_slice(t.script.events(frame));
            controllers = t.script.controllers(frame);
        }
        let start = Instant::now();
        // SPEC §content.submit: whatever finished since the last frame completes now;
        // images the shell started arrive (SPEC §machines.frames).
        if let Some(content) = &mut self.content {
            for (tag, c) in content.take_finished_tagged() {
                if c.handle == 0 && self.fetching.contains(&tag) {
                    self.fetching.retain(|&s| s != tag);
                    self.system.deliver_image(tag, image_from(c.outcome));
                } else if let Some(io) = self.io.get_mut(&tag) {
                    io.completions.push(c);
                }
            }
            // SPEC §machines.snapshots: snapshots stored, machines restored.
            let (saved, restored) = content.take_snapshot_results();
            for (slot, stored) in saved {
                self.system.snapshot_done(slot, stored);
            }
            for (tag, r) in restored {
                self.system.deliver_snapshot(tag, r);
            }
        }
        // Pasted text, as much as the focused machine's queue has room for
        // (with space for the completions' events and a page anchor), the
        // rest later.
        let focus = self.system.focus();
        let target = match focus {
            0 => Some(self.system.shell()),
            h => self.system.child(h).and_then(|c| c.machine.as_ref()),
        };
        if let Some(m) = target {
            let serial = if focus == 0 { 0 } else { self.system.child(focus).map_or(0, |c| c.serial) };
            let waiting = self.io.get(&serial).map_or(0, |io| io.completions.len());
            let room = QUEUE_CAPACITY.saturating_sub(m.system().queue().len() + self.pending.len() + waiting + 1);
            let n = room.min(self.paste.len());
            self.pending.extend(self.paste.drain(..n).map(Event::text));
        }
        let mut driver = FrameDriver {
            io: &mut self.io,
            content: &mut self.content,
            fetching: &mut self.fetching,
            screens: self.gfx.as_ref().map(|g| g.display()),
            page_thread: &mut self.page_thread,
            looks: &self.looks,
            unix_time,
            ran: Vec::new(),
            trace: self.trace,
        };
        // Panels are hit as the eye now sees them (SPEC §machines.input).
        self.system.set_eye(self.eye);
        self.system.frame(&self.pending, controllers, &mut driver);
        let ran = driver.ran;
        #[cfg(feature = "audio")]
        self.audio.frame(&mut self.system);
        self.pending.clear();
        let emulate = start.elapsed();
        let start = Instant::now();
        if let Some(gfx) = &mut self.gfx {
            let viewport = gfx.viewport();
            // The prompt names the asking machine's image (SPEC §machines.screens).
            let prompt = self.content.as_ref().and_then(|c| {
                let p = c.prompt()?;
                let title = match p.tag {
                    0 => self.shell_title.clone(),
                    tag => self.system.children().find(|(_, c)| c.serial == tag).map_or(String::new(), |(_, c)| c.title.clone()),
                };
                Some((p, c.prompts_waiting(), title))
            });
            let io = &self.io;
            let layout = |serial: u32| io.get(&serial).and_then(|io| io.page.as_ref()).and_then(|w| w.shown.clone());
            // Parallax: the eye moves towards where the pointer says, a
            // quarter of the way each frame, and rests exactly there.
            if self.looks.parallax && self.selftest.is_none() {
                let max = terse_core::vector3::MAX_EYE_OFFSET;
                let target = match self.pointer_px {
                    Some((x, y)) => {
                        let (ux, uy) = gfx.display().screen_pos(viewport, x, y);
                        let w = terse_core::SCREEN_WIDTH as f32 / 2.0;
                        let h = terse_core::SCREEN_HEIGHT as f32 / 2.0;
                        (((ux as f32 + 0.5 - w) / w * max).clamp(-max, max), ((uy as f32 + 0.5 - h) / h * max).clamp(-max, max))
                    }
                    None => self.looks.eye,
                };
                let step = |e: f32, t: f32| if (t - e).abs() < 0.05 { t } else { e + (t - e) * 0.25 };
                self.eye = (step(self.eye.0, target.0), step(self.eye.1, target.1));
            }
            gfx.display_mut().set_eye(self.eye);
            gfx.display_mut().update(&self.system, viewport, &ran, &layout, prompt);
            gfx.window.request_redraw();
        }
        if let Some(t) = &mut self.selftest {
            t.log.push(crate::selftest::FrameLog { emulate, upload: start.elapsed(), present: None });
        }
        let shell = self.system.shell();
        if let Some(fault) = shell.fault() {
            if !self.fault_shown {
                self.fault_shown = true;
                eprintln!("terse-host: fault: {fault}");
                if let Some(gfx) = &self.gfx {
                    gfx.window.set_title(&format!("{} — fault: {fault}", self.title));
                }
            }
            if self.selftest.is_some() {
                event_loop.exit();
            }
        } else if shell.stopped().is_some() {
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
        // SPEC §content.feeds: while the host asks for consent, the keyboard is the host's.
        if pressed && self.content.as_ref().is_some_and(|c| c.prompt().is_some()) {
            swallow(&mut self.swallowed, usage);
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
        // SPEC §machines.input: the home key is the host's, never a key event.
        if self.is_system() && event.physical_key == PhysicalKey::Code(HOME_KEY) {
            if pressed && !event.repeat {
                self.pending.push(Event::home());
            }
            return;
        }
        // Paste: Ctrl+V or Shift+Insert gives the clipboard's text as text events.
        let ctrl_v = self.modifiers.control_key()
            && !self.modifiers.alt_key()
            && !self.modifiers.super_key()
            && event.physical_key == PhysicalKey::Code(KeyCode::KeyV);
        let shift_insert = self.modifiers.shift_key() && event.physical_key == PhysicalKey::Code(KeyCode::Insert);
        if pressed && (ctrl_v || shift_insert) {
            swallow(&mut self.swallowed, usage);
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

    /// Queues the clipboard's text for delivery as text events (SPEC §events):
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
    /// queued as at most one event each; also the feeds-updated events.
    fn coalesced_input(&mut self) {
        // SPEC §content.events, §content.feeds: new entries were stored since the previous frame:
        // every machine with the content device is told.
        if self.content.as_mut().is_some_and(ContentService::feeds_updated) {
            for (serial, io) in self.io.iter_mut() {
                if self.system.machine_by_serial(*serial).is_some_and(|(_, m)| m.content().is_some()) {
                    io.feeds = true;
                }
            }
        }
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
    }

    fn pointer_moved(&mut self, pos: PhysicalPosition<f64>) {
        self.pointer_px = Some((pos.x, pos.y));
        let Some(gfx) = &self.gfx else { return };
        self.pointer = Some(gfx.display().screen_pos(gfx.viewport(), pos.x, pos.y));
    }

    /// winit's deltas are positive when content should move right and down,
    /// that is, when scrolling left and up; SPEC §events steps are the opposite.
    fn wheel(&mut self, delta: MouseScrollDelta) {
        let (x, y) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x as f64, y as f64),
            MouseScrollDelta::PixelDelta(p) => {
                // One step per terminal row of travel.
                let row = self.gfx.as_ref().map_or(16.0, |g| g.display().row_height(g.viewport())).max(1.0);
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
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        match Gfx::new(event_loop, &self.title, &self.looks, self.selftest.is_some()) {
            Ok(gfx) => {
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
                if !self.occluded
                    && let Some(gfx) = &mut self.gfx
                    && let Some((acquire, render)) = gfx.draw()
                {
                    if self.trace {
                        for line in gfx.display_mut().take_page_figure_log() {
                            eprintln!("page: frame {}: {line}", self.system.shell().frame());
                        }
                    }
                    if let Some(t) = &mut self.selftest {
                        t.presented(acquire, render, Instant::now());
                    }
                }
            }
            WindowEvent::Occluded(hidden) => {
                self.occluded = hidden;
                if !hidden && let Some(gfx) = &self.gfx {
                    gfx.window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => self.key(event),
            WindowEvent::CursorMoved { position, .. } => self.pointer_moved(position),
            WindowEvent::CursorLeft { .. } => self.pointer_px = None,
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
            // SPEC §frames: at most one frame per tick; if the host fell behind,
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

    /// A key the host takes (paste, the consent prompt) held down: its
    /// auto-repeated presses are noted once, so its one release is taken
    /// and nothing is left to swallow a later release.
    #[test]
    fn a_repeated_press_the_host_takes_is_released_once() {
        let mut swallowed = Vec::new();
        for _ in 0..5 {
            super::swallow(&mut swallowed, Some(0x19));
        }
        super::swallow(&mut swallowed, None);
        assert_eq!(swallowed, [0x19]);
    }
}
