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

/// How long a notice of the host's stays (frames).
const NOTICE_FRAMES: u64 = 300;
/// How long the volume shows after a change (frames).
const VOLUME_NOTICE_FRAMES: u64 = 90;

/// The host's own part, for the user.
fn host_part(c: terse_perflog::Component) -> &'static str {
    use terse_perflog::Component;
    match c {
        Component::Compose => "Drawing the picture",
        Component::Present => "Presenting the picture",
        Component::Outside => "Something outside Tersenet (the window system)",
        _ => "Tersenet's own frame work",
    }
}

/// What `TERSE_HOST_SLOW` slows, for tests of the rule (PERFORMANCE.md
/// R15): `machine<h>:<ms>:<frame>`, machine h's frames (0: the shell or the
/// machine alone) each sleep ms milliseconds from the host's frame on;
/// `compose:<ms>:<frame>`, the drawing; `:spin` after either, busy on a CPU
/// for that much CPU time instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlowPart {
    Machine(u8),
    Compose,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slow {
    what: SlowPart,
    ms: u64,
    from: u64,
    /// Busy on a CPU instead of asleep (`:spin` at the end).
    spin: bool,
}

impl Slow {
    fn parse(s: &str) -> Option<Slow> {
        let mut f = s.split(':');
        let what = match f.next()? {
            "compose" => SlowPart::Compose,
            m => SlowPart::Machine(m.strip_prefix("machine")?.parse().ok()?),
        };
        let ms = f.next()?.parse().ok()?;
        let from = f.next().map_or(Some(0), |v| v.parse().ok())?;
        let spin = match f.next() {
            None => false,
            Some("spin") => true,
            Some(_) => return None,
        };
        Some(Slow { what, ms, from, spin })
    }

    /// Takes its time: asleep, or busy on a CPU for that much CPU time
    /// (as near as the frame loop thread's clock says).
    fn take(&self) {
        let d = Duration::from_millis(self.ms);
        if !self.spin {
            return std::thread::sleep(d);
        }
        let mut probe = terse_perflog::probe::ProcProbe::this_thread();
        let start = probe.sample();
        let t = Instant::now();
        let mut x = 0u64;
        loop {
            x = std::hint::black_box(x.wrapping_add(1));
            if x % 4096 == 0 {
                let busy = match (probe.sample(), start) {
                    (Some(now), Some(s)) => Duration::from_nanos(now.since(&s).cpu_ns),
                    _ => t.elapsed(),
                };
                if busy >= d {
                    break;
                }
            }
        }
    }

    fn from_env() -> Option<Slow> {
        let v = std::env::var("TERSE_HOST_SLOW").ok()?;
        let s = Slow::parse(&v);
        if s.is_none() {
            eprintln!("terse-host: TERSE_HOST_SLOW={v} not understood (machine<h>:<ms>:<frame>[:spin] or compose:<ms>:<frame>[:spin])");
        }
        s
    }
}


/// Runs `program` in a window. With `selftest`, input comes only from its
/// script and the host measures itself (see `selftest.rs`).
pub fn run(
    program: Program,
    content: Option<ContentService>,
    selftest: Option<SelfTest>,
    drive: Option<crate::drive::Drive>,
    looks: Looks,
) -> Result<ExitCode, String> {
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let title = if program.shell { "Tersenet".to_string() } else { format!("{} — Tersenet", program.image.title_lossy()) };
    let mut system = System::new(&program.image, program.shell, program.builtins);
    system.set_shell_identity(program.identity);
    system.shell_mut().set_argument(program.argument);
    terse_net::snapshots::load_table(&mut system, &program.data);
    let looks_eye = looks.eye;
    let watch = crate::watch::HostWatch::new(
        looks.perf.clone(),
        looks.perf.is_some() && looks.perf == terse_perflog::record::default_dir(),
        match looks.renderer {
            RendererChoice::Software => "software",
            RendererChoice::Gpu => "gpu",
            RendererChoice::Auto => "auto",
        },
        if selftest.is_some() { "selftest" } else if drive.is_some() { "drive" } else { "window" },
        looks.started,
    );
    if let Some(c) = &looks.check {
        for l in c.lines() {
            watch.add(l);
        }
        if c.shed_look() {
            watch.shed(crate::phosphor::SHED_PLAIN, "check");
        }
    }
    // A self-test makes no sound unless TERSE_AUDIO says where (it runs
    // unattended, often in a headless compositor); nor does a host told
    // TERSE_AUDIO=off. A drive never does.
    let audio = crate::audio::Audio::new(if drive.is_some() {
        crate::audio::Choice::Off
    } else {
        crate::audio::Choice::from_env(selftest.is_some())
    });
    let mut app = App {
        system,
        io: HashMap::new(),
        content,
        fetching: Vec::new(),
        page_thread: None,
        governor: crate::phosphor::Governor::at(looks.look.floor),
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
        presented_at: None,
        occluded: false,
        ticks: 0,
        hold: crate::hold::ResizeHold::default(),
        selftest,
        refresh_hz: None,
        trace: std::env::var_os("TERSE_PAGE_TRACE").is_some(),
        fault_shown: false,
        watch,
        data: program.data.clone(),
        slow: Slow::from_env(),
        notice: None,
        halted: None,
        monitor: None,
        drive,
        used: Vec::new(),
        audio,
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    if let Some(e) = app.error {
        return Err(e);
    }
    if let Some(t) = &app.selftest {
        t.report(app.refresh_hz);
    }
    if let Some(d) = &mut app.drive {
        d.finish();
    }
    // Stopped by the host (PERFORMANCE.md R15): exit status 4 (STATE.md 6.4).
    if let Some((slot, saved)) = &app.halted {
        eprintln!("terse-host: stopped by the host: device {slot} did not keep up; its state before the stop: {saved}");
        return Ok(ExitCode::from(4));
    }
    if app.selftest.is_some() {
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(code) = app.drive.as_ref().and_then(|d| d.status()) {
        return Ok(code);
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
    fn new(event_loop: &ActiveEventLoop, title: &str, looks: &Looks, test: bool, capture: bool) -> Result<Self, String> {
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
                match Self::gpu(event_loop, &window, glyphs.clone(), size, auto, capture) {
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
        capture: bool,
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
        // `--drive` reads back what is presented, if the surface allows.
        let usage = if capture && caps.usages.contains(wgpu::TextureUsages::COPY_SRC) {
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC
        } else {
            wgpu::TextureUsages::RENDER_ATTACHMENT
        };
        let config = wgpu::SurfaceConfiguration {
            usage,
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

    /// A held frame period's picture (hold.rs): the software renderer's last
    /// frame scaled to the window (nearest pixel), nothing drawn again; the
    /// GPU draws the screens as they are at the window's size (the machines
    /// held).
    fn draw_held(&mut self, capture: bool) -> Option<(Duration, Duration, Option<crate::drive::Raw>)> {
        match &mut self.presenter {
            #[cfg(feature = "gpu")]
            Presenter::Gpu { .. } => self.draw(capture),
            Presenter::Soft { surface, display, size, history } => {
                if size.0 == 0 || size.1 == 0 {
                    return None;
                }
                let start = Instant::now();
                let frame = display.frame();
                let (fw, fh) = (frame.area.width(), frame.area.height());
                let mut buffer = surface.buffer_mut().ok()?;
                let acquired = Instant::now();
                let (w, h) = (size.0 as usize, size.1 as usize);
                if buffer.len() < w * h {
                    return None;
                }
                crate::hold::scale_nearest(&frame.data, (fw, fh), &mut buffer[..w * h], (w, h));
                let picture = capture.then(|| crate::drive::Raw::Packed(buffer[..w * h].to_vec()));
                // The next frame drawn is copied whole.
                history.clear();
                self.window.pre_present_notify();
                if let Err(e) = buffer.present() {
                    eprintln!("terse-host: softbuffer: {e}");
                }
                Some((acquired - start, acquired.elapsed(), picture))
            }
        }
    }

    /// Draws and presents; returns how long acquiring the swapchain image
    /// (or buffer) took and how long the rest (draw, present) took, and
    /// with `capture` the picture presented (RGBA, `--drive`).
    fn draw(&mut self, capture: bool) -> Option<(Duration, Duration, Option<crate::drive::Raw>)> {
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
                let readback = (capture && config.usage.contains(wgpu::TextureUsages::COPY_SRC))
                    .then(|| crate::screens::copy_out(&display.device, &mut encoder, &frame.texture, viewport));
                display.queue.submit([encoder.finish()]);
                self.window.pre_present_notify();
                display.queue.present(frame);
                let took = acquired.elapsed();
                let picture = match readback {
                    Some(r) => crate::screens::read_out(&display.device, r, viewport, config.format).ok(),
                    // A surface that cannot be read: the same picture drawn offscreen.
                    None if capture => display.snapshot(viewport).ok().map(|data| crate::drive::Raw::Rows { data, stride: viewport.0 as usize * 4, bgra: false }),
                    None => None,
                };
                Some((acquired - start, took, picture))
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
                let took = (drawn - start) + acquired.elapsed();
                Some((acquired - drawn, took, capture.then(|| crate::drive::Raw::Packed(display.frame().data.clone()))))
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
    /// Times each machine's frame (PERFORMANCE.md R14).
    watch: &'a mut crate::watch::HostWatch,
    slow: Option<Slow>,
    /// Each machine's budget use this frame (serial, units).
    used: Vec<(u32, u64)>,
}

impl Driver for FrameDriver<'_> {
    fn frame(&mut self, who: Who, machine: &mut Machine, events: Vec<Event>, controllers: [Controller; CONTROLLERS]) -> FrameResult {
        self.watch.machine_begin();
        let t0 = Instant::now();
        let mut page_time = Duration::ZERO;
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
        let t = Instant::now();
        if let Some(worker) = &mut io.page
            && let Ok(page_events) = worker.poll(machine.page())
            && !page_events.is_empty()
        {
            if self.trace {
                eprintln!("page: machine {}: frame {}: {} page events queued", who.handle, machine.frame(), page_events.len());
            }
            input.splice(0..0, page_events);
        }
        page_time += t.elapsed();
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
        self.used.push((who.serial, result.used));
        // The test hook (TERSE_HOST_SLOW): this machine's frames take longer.
        if let Some(s @ Slow { what: SlowPart::Machine(h), from, .. }) = self.slow
            && h == who.handle
            && self.watch.frame >= from
        {
            s.take();
        }
        let t = Instant::now();
        if let Some(worker) = &mut io.page
            && let Some(e) = worker.after_frame(machine.page())
        {
            io.carry.push(e);
        }
        page_time += t.elapsed();
        if getrandom::fill(&mut io.random[..result.random_used]).is_err() {
            io.random[..result.random_used].fill(0);
        }
        let t = Instant::now();
        if let Some(content) = self.content.as_mut() {
            content.submit_machine(who.serial, machine);
        }
        let content_time = t.elapsed();
        self.ran.push(who.serial);
        let all = t0.elapsed();
        self.watch.machine_end(who.serial, *machine.image_id(), [all.saturating_sub(page_time + content_time), page_time, content_time]);
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
    /// Sheds the phosphor look when frames run long (`phosphor::Governor`).
    governor: crate::phosphor::Governor,
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
    /// When the last picture was presented: a frame that begins after its
    /// tick because the last one ran long is late by that one's doing,
    /// already counted there; only the rest is `outside` (R14).
    presented_at: Option<Instant>,
    /// The window is hidden (another workspace, minimised): nothing is drawn.
    occluded: bool,
    /// Frame periods so far, run or held, and the hold while the window's
    /// size changes (PERFORMANCE.md R15, "Resizing"; `hold`, as terse-bus).
    ticks: u64,
    hold: crate::hold::ResizeHold,
    selftest: Option<SelfTest>,
    refresh_hz: Option<f64>,
    /// `TERSE_PAGE_TRACE`: print when page layouts are asked for and arrive.
    trace: bool,
    fault_shown: bool,
    /// The watch over the host's frames (PERFORMANCE.md R14, R15).
    watch: crate::watch::HostWatch,
    /// The data directory: the shell's snapshots, the automatic slots.
    data: std::path::PathBuf,
    /// `TERSE_HOST_SLOW`, for tests.
    slow: Option<Slow>,
    /// The host's notice (R15) and the host frame after which it goes.
    notice: Option<(crate::consent::Notice, u64)>,
    /// The device whose slowness stopped the shell or the machine alone,
    /// and where its snapshot went: the run ends with status 4.
    halted: Option<(u8, String)>,
    /// The monitor the window was last on (a change excuses frames).
    monitor: Option<winit::monitor::MonitorHandle>,
    /// `--drive`: the script driving the window, and watching it.
    drive: Option<crate::drive::Drive>,
    /// Each machine's budget use in the last frame (serial, units).
    used: Vec<(u32, u64)>,
    /// The sound device's output (SPEC §sound.presentation), opened when a machine
    /// first gives samples.
    /// The sound (SPEC §sound.presentation).
    audio: crate::audio::Audio,
}

use terse_net::service::image_from;

impl App {
    /// Whether the system is a shell with machines (the home key is the
    /// host's), not a single machine.
    fn is_system(&self) -> bool {
        self.system.is_system()
    }

    /// A frame period held while the window's size changes: no machine
    /// runs; the script's actions (`--drive`) go on; the last picture is
    /// shown at the window's size when it is drawn (`Gfx::draw`).
    fn held(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(mut d) = self.drive.take() {
            let actions = d.before_frame(self.system.shell().frame());
            for a in actions {
                self.drive_action(a, event_loop);
            }
            self.drive = Some(d);
        }
        if let Some(gfx) = &self.gfx {
            gfx.window.request_redraw();
        }
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
        if let Some(mut d) = self.drive.take() {
            let actions = d.before_frame(self.system.shell().frame());
            for a in actions {
                self.drive_action(a, event_loop);
            }
            self.drive = Some(d);
        }
        self.coalesced_input();
        let mut controllers = self.gamepads.poll();
        if let Some(c) = self.drive.as_ref().and_then(|d| d.controllers()) {
            controllers = c;
        }
        if let Some(t) = &self.selftest {
            // The script is the only input (coalesced_input sent nothing).
            let frame = self.system.shell().frame();
            self.pending.extend_from_slice(t.script.events(frame));
            controllers = t.script.controllers(frame);
        }
        let start = Instant::now();
        let late_start = start.saturating_duration_since(self.presented_at.map_or(self.next_frame, |p| p.max(self.next_frame)));
        self.watch.begin();
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
            watch: &mut self.watch,
            slow: self.slow,
            used: Vec::new(),
        };
        // Panels are hit as the eye now sees them (SPEC §machines.input).
        self.system.set_eye(self.eye);
        self.system.frame(&self.pending, controllers, &mut driver);
        let ran = driver.ran;
        self.used = driver.used;
        if let Some(mut d) = self.drive.take() {
            d.after_frame(&*self, &self.used);
            self.drive = Some(d);
        }
        self.sound();
        self.pending.clear();
        self.watch.emulated();
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
            // quarter of the way each frame, and rests exactly there; over a
            // machine that lists the 3D display it goes back to rest
            // (spec/vector3d.md §vector3d.space).
            if self.looks.parallax && self.selftest.is_none() {
                let max = terse_core::vector3::MAX_EYE_OFFSET;
                let target = match self.pointer_px {
                    Some((x, y)) if !self.system.eye_may_follow(gfx.display().screen_pos(viewport, x, y)) => self.looks.eye,
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
        self.watch.stepped(emulate, start.elapsed(), late_start);
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

    /// The window moved to another monitor, or its scale changed: as a
    /// size change, the next frames are excused (PERFORMANCE.md R15).
    fn monitor_changed(&mut self) {
        self.watch.resized();
        let (width, height) = self.gfx.as_ref().map_or((0, 0), |g| g.viewport());
        self.watch.add(terse_perflog::Line::Resize { frame: self.watch.frame, width, height });
    }

    /// Machine `h`'s name for the user.
    fn machine_name(&self, h: u8) -> String {
        match h {
            0 if self.is_system() => "the shell".to_string(),
            0 => format!("“{}”", self.shell_title),
            h => format!("machine {h} (“{}”)", self.system.child(h).map_or("", |c| c.title.as_str())),
        }
    }

    fn show_notice(&mut self, heading: &str, what: String, then: &str) {
        self.show_notice_for(heading, what, then, NOTICE_FRAMES);
    }

    fn show_notice_for(&mut self, heading: &str, what: String, then: &str, frames: u64) {
        let n = crate::consent::Notice { heading: heading.into(), what, then: then.into() };
        if let Some(gfx) = &mut self.gfx {
            gfx.display_mut().set_notice(Some(n.clone()));
        }
        self.notice = Some((n, self.watch.frame + frames));
    }

    /// The frame's sound to the output; if it cannot be heard, the user
    /// is told (once, and again when the cause changes).
    fn sound(&mut self) {
        let mut frame = [0i16; 2 * terse_core::sound::FRAME_SAMPLES];
        let any = self.system.take_audio(&mut frame);
        if let Some(s) = self.audio.frame(any.then_some(&frame[..])) {
            eprintln!("terse-host: {}", s.line());
            let (heading, what, then) = s.notice();
            self.show_notice(&heading, what, &then);
        }
    }

    fn expire_notice(&mut self) {
        if self.notice.as_ref().is_some_and(|(_, until)| *until < self.watch.frame) {
            self.notice = None;
            if let Some(gfx) = &mut self.gfx {
                gfx.display_mut().set_notice(None);
            }
        }
    }

    /// Carries out what the rule asks (PERFORMANCE.md R15), as terse-bus
    /// does: shed the look; warn and save the machine at once; save and
    /// stop it. The host's own parts are warned about and, with nothing
    /// left to shed and nobody to stop, recorded as not keeping up.
    fn act(&mut self, event_loop: &ActiveEventLoop, acts: Vec<crate::rule::Act>) {
        use crate::rule::{Act, Culprit};
        use terse_perflog::{Component, Line};
        let frame = self.watch.frame.saturating_sub(1);
        for a in acts {
            match a {
                Act::Shed => {
                    if let Some(floor) = self.governor.shed() {
                        self.looks.look.floor = floor;
                        self.watch.shed(floor, "late");
                        if let Some(gfx) = &mut self.gfx {
                            gfx.display_mut().set_look(self.looks.look);
                        }
                        eprintln!("terse-host: the drawing made a frame late: the phosphor look sheds from level {floor}");
                        self.note_slow(format!("the look shed from level {floor}"));
                    }
                }
                Act::Warn { culprit: Culprit::Machine(serial), count } => {
                    let Some(h) = crate::halt::handle(&self.system, serial) else { continue };
                    let what = self.machine_name(h);
                    self.watch.add(Line::Warn { frame, component: Component::Core, count });
                    // Save at once, while it still runs.
                    let (_, saved) = crate::halt::save(&mut self.system, &self.data, self.watch.dir.as_deref(), h);
                    eprintln!("terse-host: {what} is slow ({count} late frames): your work is saved ({saved}); if it goes on, it is stopped");
                    self.note_slow(format!("warned: {what} is slow ({count} late frames)"));
                    self.show_notice("a slow machine", format!("{what} is slow: {count} late frames."), "Its work is saved; if it goes on, it is stopped.");
                }
                Act::Stop { culprit: Culprit::Machine(serial), count } => {
                    self.watch.rule.forget(Culprit::Machine(serial));
                    let Some(h) = crate::halt::handle(&self.system, serial) else { continue };
                    let what = self.machine_name(h);
                    let slot = self.watch.slot_of(serial);
                    let (saved, d) = crate::halt::save(&mut self.system, &self.data, self.watch.dir.as_deref(), h);
                    let image = self.system.machine_by_serial(serial).map(|(_, m)| *m.image_id());
                    self.watch.add(Line::Stop { frame, component: Component::Core, machine: h as u32, image: terse_perflog::record::image_field(image.as_ref()), saved });
                    crate::halt::halt(&mut self.system, h, slot);
                    eprintln!("terse-host: {what} did not keep up ({count} late frames): machine {h} stopped; {d}");
                    self.note_slow(format!("stopped: {what} did not keep up ({count} late frames)"));
                    self.show_notice("a machine did not keep up", format!("{what} did not keep up: {count} late frames."), "It was saved and stopped.");
                    if h == 0 {
                        self.halted = Some((slot, d));
                        event_loop.exit();
                    }
                }
                Act::Warn { culprit: Culprit::Host(c), count } => {
                    self.watch.add(Line::Warn { frame, component: c, count });
                    let what = host_part(c);
                    eprintln!("terse-host: {what} made {count} frames late (`terse perf log` shows the record)");
                    self.note_slow(format!("warned: {what} made {count} frames late"));
                    self.show_notice("this computer is slow", format!("{what} made {count} frames late."), "A smaller window may help; the record shows it (terse perf log).");
                }
                Act::Stop { culprit: Culprit::Host(c), count } => {
                    // Nothing left to shed and nobody else to stop: said
                    // plainly, recorded, and counted against this host.
                    self.watch.add(Line::fail(frame, c, count));
                    let what = host_part(c);
                    eprintln!("terse-host: {what} does not keep up on this computer ({count} late frames, nothing left to shed): this terse-host fails PERFORMANCE.md R15 here; `terse-host --hardware-check` and `terse perf` show why");
                    self.note_slow(format!("failed: {what} does not keep up ({count} late frames)"));
                    self.show_notice("Tersenet does not keep up here", format!("{what} does not keep up on this computer: {count} late frames."), "Recorded as this host's failure. A smaller window may help.");
                }
            }
        }
    }

    fn key(&mut self, event: KeyEvent) {
        if self.selftest.is_some() || self.drive.is_some() {
            return;
        }
        let pressed = event.state == ElementState::Pressed;
        self.key_input(event.physical_key, pressed, event.repeat, event.text.as_deref(), prompt_answer(&event));
    }

    /// A key from the window system (or `--drive`): its physical key,
    /// whether pressed, auto-repeated, the text it types, and the answer it
    /// gives the consent prompt.
    fn key_input(&mut self, physical_key: PhysicalKey, pressed: bool, repeat: bool, text: Option<&str>, answer: Option<bool>) {
        let usage = match physical_key {
            PhysicalKey::Code(code) => hid_usage(code),
            _ => None,
        };
        // SPEC §content.feeds: while the host asks for consent, the keyboard is the host's.
        if pressed && self.content.as_ref().is_some_and(|c| c.prompt().is_some()) {
            swallow(&mut self.swallowed, usage);
            if !repeat
                && let Some(allow) = answer
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
        if self.is_system() && physical_key == PhysicalKey::Code(HOME_KEY) {
            if pressed && !repeat {
                self.pending.push(Event::home());
            }
            return;
        }
        // The volume: the host's keys, never a program's.
        if let PhysicalKey::Code(code) = physical_key
            && let Some(k) = crate::keymap::volume_key(code, self.modifiers)
        {
            if pressed {
                swallow(&mut self.swallowed, usage);
                let what = self.audio.change(k);
                self.show_notice_for("sound", what, "Ctrl+Alt+minus and plus change it, Ctrl+Alt+M mutes, for this run.", VOLUME_NOTICE_FRAMES);
            }
            return;
        }
        // Paste: Ctrl+V or Shift+Insert gives the clipboard's text as text events.
        let ctrl_v = self.modifiers.control_key()
            && !self.modifiers.alt_key()
            && !self.modifiers.super_key()
            && physical_key == PhysicalKey::Code(KeyCode::KeyV);
        let shift_insert = self.modifiers.shift_key() && physical_key == PhysicalKey::Code(KeyCode::Insert);
        if pressed && (ctrl_v || shift_insert) {
            swallow(&mut self.swallowed, usage);
            self.paste_clipboard();
            return;
        }
        if let Some(usage) = usage {
            let mods = modifier_bits(self.modifiers);
            self.pending.push(if pressed { Event::key_down(usage, mods) } else { Event::key_up(usage, mods) });
        }
        if pressed && let Some(text) = text {
            self.pending.extend(text.chars().filter(|c| !c.is_control()).map(Event::text));
        }
    }

    /// The host noted a slow frame: `--drive` reports it.
    fn note_slow(&mut self, note: String) {
        if let Some(d) = &mut self.drive {
            d.slow_frame(note);
        }
    }

    /// Carries out what `--drive` asks, as the window system's events would.
    fn drive_action(&mut self, a: crate::drive::Action, event_loop: &ActiveEventLoop) {
        use crate::drive::Action;
        match a {
            Action::Modifiers(bits) => {
                use terse_core::modifiers::*;
                let mut m = ModifiersState::empty();
                for (bit, s) in [(SHIFT, ModifiersState::SHIFT), (CONTROL, ModifiersState::CONTROL), (ALT, ModifiersState::ALT), (META, ModifiersState::SUPER)] {
                    if bits & bit != 0 {
                        m |= s;
                    }
                }
                self.modifiers = m;
            }
            Action::Key { usage, pressed, repeat, text } => {
                let Some(code) = crate::keymap::key_code(usage) else { return };
                let answer = match code {
                    KeyCode::KeyY => Some(true),
                    KeyCode::KeyN | KeyCode::Escape => Some(false),
                    _ => None,
                };
                self.key_input(PhysicalKey::Code(code), pressed, repeat, text.as_deref(), answer);
            }
            // As a paste: no faster than the focused machine's queue has room for.
            Action::Text(t) => self.paste.extend(t.chars()),
            Action::Pointer { x, y, px } => {
                let pos = if px {
                    PhysicalPosition::new(x, y)
                } else {
                    // The centre of the screen unit, in window pixels.
                    let Some(gfx) = &self.gfx else { return };
                    let (ox, oy, w, _) = gfx.display().screen_rect(gfx.viewport());
                    let s = w as f64 / terse_core::SCREEN_WIDTH as f64;
                    PhysicalPosition::new(ox as f64 + (x + 0.5) * s, oy as f64 + (y + 0.5) * s)
                };
                self.pointer_moved(pos);
            }
            Action::Buttons(b) => {
                for (bit, button) in [(buttons::PRIMARY, MouseButton::Left), (buttons::SECONDARY, MouseButton::Right), (buttons::MIDDLE, MouseButton::Middle)] {
                    if (self.buttons & bit != 0) != (b & bit != 0) {
                        self.mouse_button(if b & bit != 0 { ElementState::Pressed } else { ElementState::Released }, button);
                    }
                }
            }
            Action::Wheel { v, h } => self.wheel(MouseScrollDelta::LineDelta(-h as f32, -v as f32)),
            Action::Event(e) => self.pending.push(e),
            Action::Resize(w, h) => {
                // A size the window system applies at once comes back here
                // rather than as a Resized event (winit on Wayland).
                let applied = self.gfx.as_ref().and_then(|g| g.window.request_inner_size(PhysicalSize::new(w, h)));
                if let Some(size) = applied {
                    self.window_event(event_loop, self.gfx.as_ref().unwrap().window.id(), WindowEvent::Resized(size));
                }
            }
            Action::Quit => event_loop.exit(),
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

/// What `--drive` reads: the machines' devices and the page as this host
/// laid it out.
impl crate::drive::Probe for App {
    fn viewport(&self) -> (u32, u32) {
        self.gfx.as_ref().map_or((0, 0), |g| g.viewport())
    }

    fn dump(&self, what: &str, handle: Option<u8>) -> Result<String, String> {
        if what == "state" {
            return Ok(self.state_dump());
        }
        let h = handle.unwrap_or_else(|| self.system.focus());
        let (machine, serial) = match h {
            0 => (self.system.shell(), 0),
            h => {
                let c = self.system.child(h).ok_or(format!("no machine {h}"))?;
                (c.machine.as_ref().ok_or(format!("machine {h} has not started"))?, c.serial)
            }
        };
        match what {
            "terminal" => machine.terminal().map(|t| t.text()).ok_or_else(|| "no terminal device".to_string()),
            "vectors" => {
                let mut out = String::new();
                if let Some(line) = machine.vector().and_then(|v| v.style_line()) {
                    out += &format!("{line}\n");
                }
                match machine.vector() {
                    Some(v) => out += &format!("-- vectors: {} list bytes --\n{}", machine.vector_list().len(), v.dump()),
                    None => out += "-- no vector display --\n",
                }
                if let Some(v) = machine.vector3() {
                    out += &v.dump();
                }
                Ok(out)
            }
            "page" => {
                let page = machine.page().ok_or("no page device")?;
                let layout = self.io.get(&serial).and_then(|io| io.page.as_ref()).and_then(|w| w.shown.clone());
                Ok(match layout {
                    None => "-- page: nothing laid out yet --\n".to_string(),
                    Some(l) => {
                        let scroll = page.scroll_for(l.reported_height()) as f32;
                        let r = page.region();
                        format!("-- page region {} {} {} {}, document generation {} (shown: {}) --\n{}", r.x, r.y, r.w, r.h, page.generation(), l.key.0, l.dump(scroll, r.h as f32))
                    }
                })
            }
            w => Err(format!("no dump {w}")),
        }
    }
}

impl App {
    /// `dump state`: the frame, the window, the machines, the budget.
    fn state_dump(&self) -> String {
        let viewport = self.gfx.as_ref().map_or((0, 0), |g| g.viewport());
        let rect = self.gfx.as_ref().map_or((0.0, 0.0, 0.0, 0.0), |g| g.display().screen_rect(viewport));
        let name = self.gfx.as_ref().map_or(String::new(), |g| g.display().name());
        let mut s = format!(
            "frame {}, window {}x{}, screen at {},{} size {}x{}, renderer {name}\nfocus {}\n",
            self.system.shell().frame(),
            viewport.0,
            viewport.1,
            rect.0,
            rect.1,
            rect.2,
            rect.3,
            self.system.focus()
        );
        let shell = self.system.shell();
        let total = terse_core::BUDGET_PER_FRAME as f64;
        let used = |serial: u32| self.used.iter().find(|u| u.0 == serial).map_or(String::from("-"), |u| format!("{:.1}%", u.1 as f64 / total * 100.0));
        s += &format!("machine 0 \"{}\" ({}), last frame's budget {}{}\n", self.shell_title, if self.is_system() { "shell" } else { "alone" }, used(0), shell.stopped().map_or(String::new(), |st| format!(", stopped: {st:?}")));
        for (h, c) in self.system.children() {
            s += &format!("machine {h} \"{}\" serial {}, state {}, last frame's budget {}\n", c.title, c.serial, self.system.state(h), used(c.serial));
        }
        s
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        match Gfx::new(event_loop, &self.title, &self.looks, self.selftest.is_some() || self.drive.is_some(), self.drive.is_some()) {
            Ok(gfx) => {
                self.refresh_hz = gfx
                    .window
                    .current_monitor()
                    .and_then(|m| m.refresh_rate_millihertz())
                    .map(|mhz| mhz as f64 / 1000.0);
                self.monitor = gfx.window.current_monitor();
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
                    // A real change of the surface's size: the next frames
                    // are excused (PERFORMANCE.md R15).
                    if size.width > 0 && size.height > 0 && gfx.viewport() != (size.width, size.height) {
                        self.hold.resized(self.ticks);
                        self.watch.resized();
                        self.watch.add(terse_perflog::Line::Resize { frame: self.watch.frame, width: size.width, height: size.height });
                    }
                    gfx.resize(size);
                    gfx.window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => self.monitor_changed(),
            WindowEvent::Moved(_) => {
                let now = self.gfx.as_ref().and_then(|g| g.window.current_monitor());
                if now.is_some() && self.monitor.is_some() && now != self.monitor {
                    self.monitor_changed();
                }
                if now.is_some() {
                    self.monitor = now;
                }
            }
            WindowEvent::RedrawRequested => {
                self.watch.drawing();
                // The test hook (TERSE_HOST_SLOW): the drawing takes longer.
                let slept = match self.slow {
                    Some(s @ Slow { what: SlowPart::Compose, from, .. }) if self.watch.frame >= from && !self.occluded => {
                        let t = Instant::now();
                        s.take();
                        t.elapsed()
                    }
                    _ => Duration::ZERO,
                };
                // The window's size as the window system has it now: a size
                // whose Resized never came (or came while the surface was
                // being reconfigured) is taken here, so the picture is never
                // left at an old size.
                if let Some(gfx) = &mut self.gfx {
                    let s = gfx.window.inner_size();
                    if s.width > 0 && s.height > 0 && gfx.viewport() != (s.width, s.height) {
                        self.hold.resized(self.ticks);
                        self.watch.resized();
                        self.watch.add(terse_perflog::Line::Resize { frame: self.watch.frame, width: s.width, height: s.height });
                        gfx.resize(s);
                    }
                }
                let capture = self.drive.as_ref().is_some_and(crate::drive::Drive::wants_picture);
                let holding = self.hold.holding();
                if !self.occluded
                    && let Some(gfx) = &mut self.gfx
                    && let Some((acquire, render, picture)) = if holding { gfx.draw_held(capture) } else { gfx.draw(capture) }
                {
                    if holding {
                        // Not a frame: nothing is judged, nothing sheds.
                        if let Some(mut d) = self.drive.take() {
                            d.presented(self.system.shell().frame(), picture, &*self);
                            self.drive = Some(d);
                        }
                        return;
                    }
                    let render = render + slept;
                    self.presented_at = Some(Instant::now());
                    if self.trace {
                        for line in gfx.display_mut().take_page_figure_log() {
                            eprintln!("page: frame {}: {line}", self.system.shell().frame());
                        }
                    }
                    if let Some(t) = &mut self.selftest {
                        t.presented(acquire, render, Instant::now());
                    }
                    // A frame that took longer than a frame to draw sheds
                    // more of the look (presentation only).
                    if !self.looks.look.no_shed
                        && let Some(floor) = self.governor.presented(render)
                    {
                        let why = if floor > self.looks.look.floor { "late" } else { "recover" };
                        self.looks.look.floor = floor;
                        self.watch.shed(floor, why);
                        gfx.display_mut().set_look(self.looks.look);
                        eprintln!("terse-host: frames ran long or caught up: the phosphor look sheds from level {floor}");
                    }
                    let can_shed = !self.looks.look.no_shed && self.governor.can_shed();
                    if let Some(judged) = self.watch.presented(acquire, render, can_shed) {
                        if self.drive.is_some()
                            && let Some(l) = judged.late.as_ref().filter(|l| judged.excused.is_none() && l.verdict == terse_perflog::Verdict::Blamed)
                        {
                            // Late by this host's or a machine's doing (not
                            // starved by other work on the computer).
                            self.note_slow(format!("late frame {}: {:?} took {:.1} ms", l.frame, l.top.component, l.wall_ns as f64 / 1e6));
                        }
                        if self.looks.rule {
                            self.act(event_loop, judged.acts);
                        }
                    }
                    self.expire_notice();
                    if let Some(mut d) = self.drive.take() {
                        d.presented(self.system.shell().frame(), picture, &*self);
                        self.drive = Some(d);
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
            self.ticks += 1;
            // While the size changes the machines are held and the last
            // picture is shown scaled (hold.rs), as terse-bus does.
            if self.hold.step(self.ticks) {
                self.held(event_loop);
            } else {
                self.step(event_loop);
            }
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
