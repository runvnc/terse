//! The composed picture in a window (softbuffer), and the window's input as
//! the machines' events (SPEC §events): keys (F12 is the home key), text, paste
//! (Ctrl+V, Shift+Insert), the pointer in the shell's screen units (the
//! system maps it to tiles and panels, SPEC §machines.input), the wheel; the eye
//! follows the pointer (parallax). `--selftest` runs a number of frames
//! with the scripted input only and prints the emulation, bus,
//! composition and present-interval timings.

use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use terse_bus::compose::Compositor;
use terse_bus::host::BusSystem;
use terse_bus::signals;
use terse_bus::stats::spread;
use terse_core::system::{QUEUE_CAPACITY, buttons};
use terse_core::{Controller, Event, Script};
use terse_host::display::screen_pos;
use terse_host::keymap::{HOME_KEY, MAX_PASTE, hid_usage, modifier_bits, paste_chars};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowId};

const FRAME: Duration = Duration::from_micros(16_667);

/// While the window's surface is changing size, the machines are held (no
/// frame runs, nothing is drawn; the last picture is shown scaled), as
/// terse-host holds them (`terse_host::hold`, PERFORMANCE.md R15,
/// "Resizing").
// (`SETTLE`, `MAX_HOLD`, `RUN_BETWEEN` there.)

pub struct Options {
    pub size: Option<(u32, u32)>,
    pub eye: (f32, f32),
    pub parallax: bool,
    /// `--selftest`: run this many frames with the script only.
    pub selftest: Option<u64>,
    pub threads: usize,
    pub flat: bool,
    pub startup: Duration,
    /// How the phosphor style is drawn.
    pub looks: terse_bus::compose::Looks,
    /// `--resize` (with --selftest): the window's surface takes these sizes
    /// before these frames, as when the window system configures it (a
    /// script of size changes, for testing R15).
    pub resizes: Vec<(u64, (u32, u32))>,
}

/// One frame's timings (ms).
#[derive(Default, Clone)]
struct FrameLog {
    emulate: f64,
    bus: f64,
    /// Composition: the drivers' PRESENT waits and the core's own work.
    compose: f64,
    /// Each driver's PRESENT, from sending it to its reply (by driver).
    drivers: Vec<f64>,
    present: f64,
    /// Of `present`, waiting for the compositor to release a buffer.
    buffer_wait: f64,
    /// The compositor's own account of the draw.
    profile: terse_bus::compose::Profile,
}

struct App {
    bs: BusSystem,
    script: Script,
    comp: Compositor,
    opts: Options,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    viewport: (u32, u32),
    pending: Vec<Event>,
    pointer: Option<(u16, u16)>,
    pointer_sent: Option<(u16, u16)>,
    pointer_px: Option<(f64, f64)>,
    buttons: u8,
    wheel: (f64, f64),
    modifiers: ModifiersState,
    paste: VecDeque<char>,
    clipboard: Option<arboard::Clipboard>,
    eye: (f32, f32),
    next: Instant,
    error: Option<String>,
    frames: u64,
    log: Vec<FrameLog>,
    presents: Vec<Instant>,
    first_frame: Option<Duration>,
    /// When the window side began (the drivers had started).
    created: Instant,
    /// Sheds the phosphor look when frames run long.
    governor: terse_host::phosphor::Governor,
    /// The last presents' frame sizes and damage, newest first (what a
    /// buffer of an age is missing).
    presented: VecDeque<((u32, u32), Vec<terse_host::soft::base::IRect>)>,
    /// The last present's wait for a buffer the compositor had released (ms).
    buffer_wait: f64,
    /// The highest shed level said on standard error.
    shed_said: u8,
    /// `--resize`: the window's surface sizes before these frame periods.
    resizes: VecDeque<(u64, (u32, u32))>,
    /// Frame periods so far (run or held).
    ticks: u64,
    /// Holding the machines while the surface changes size.
    hold: terse_host::hold::ResizeHold,
    /// The sound (SPEC §sound.presentation), played as terse-host plays it.
    audio: terse_host::audio::Audio,
    /// Why sound cannot be heard, shown until this frame.
    sound_notice: Option<((String, String, String), u64)>,
}

impl App {
    /// The bus's round trips so far (ms), summed over the drivers.
    fn bus_ms(&self) -> f64 {
        let bus = self.bs.bus.borrow();
        bus.procs
            .iter()
            .map(|d| {
                let h = [&d.stats.calls, &d.stats.begin];
                h.iter().map(|x| x.sum_ns as f64).sum::<f64>() / 1e6
            })
            .sum()
    }

    /// Each driver's PRESENT waits so far (ms).
    fn present_ms(&self) -> Vec<f64> {
        self.bs.bus.borrow().procs.iter().map(|d| d.stats.present.sum_ns as f64 / 1e6).collect()
    }

    fn frame(&mut self) -> Result<(), String> {
        let shell_frame = self.bs.system.shell().frame();
        let mut events = std::mem::take(&mut self.pending);
        let mut controllers = [Controller::default(); terse_core::CONTROLLERS];
        if self.opts.selftest.is_some() {
            events = self.script.events(shell_frame).to_vec();
            controllers = self.script.controllers(shell_frame);
        } else {
            events.extend_from_slice(self.script.events(shell_frame));
            // Pasted text, as much as the focused machine's queue has room for.
            let room = QUEUE_CAPACITY.saturating_sub(events.len() + 8);
            let n = room.min(self.paste.len());
            events.extend(self.paste.drain(..n).map(Event::text));
        }
        // SPEC §content.feeds: while a question is asked the keyboard is the host's.
        let events = self.bs.input(events)?;
        let bus_before = self.bus_ms();
        let start = Instant::now();
        self.bs.system.set_eye(self.eye);
        let done = self.bs.frame(&events, controllers)?;
        let emulate = start.elapsed().as_secs_f64() * 1000.0;
        self.comp.ran(&done, &self.bs.system);
        let bus = self.bus_ms() - bus_before;
        let mut sound = [0i16; 2 * terse_core::sound::FRAME_SAMPLES];
        let any = self.bs.system.take_audio(&mut sound);
        if let Some(s) = self.audio.frame(any.then_some(&sound[..])) {
            eprintln!("terse-bus: {}", s.line());
            self.sound_notice = Some((s.notice(), self.frames + terse_bus::host::NOTICE_FRAMES));
        }
        if self.sound_notice.as_ref().is_some_and(|(_, until)| *until < self.frames) {
            self.sound_notice = None;
        }
        // Parallax: the eye moves towards where the pointer says, a quarter
        // of the way each frame, and rests exactly there.
        if self.opts.parallax && self.opts.selftest.is_none() {
            let max = terse_core::vector3::MAX_EYE_OFFSET;
            // Over a machine that lists the 3D display the eye rests
            // (spec/vector3d.md §vector3d.space).
            let target = match self.pointer_px {
                Some((x, y)) if !self.bs.system.eye_may_follow(screen_pos(self.comp.shell_rect(self.viewport), x, y)) => self.opts.eye,
                Some((x, y)) => {
                    let (ux, uy) = screen_pos(self.comp.shell_rect(self.viewport), x, y);
                    let (w, h) = (terse_core::SCREEN_WIDTH as f32 / 2.0, terse_core::SCREEN_HEIGHT as f32 / 2.0);
                    (((ux as f32 + 0.5 - w) / w * max).clamp(-max, max), ((uy as f32 + 0.5 - h) / h * max).clamp(-max, max))
                }
                None => self.opts.eye,
            };
            let step = |e: f32, t: f32| if (t - e).abs() < 0.05 { t } else { e + (t - e) * 0.25 };
            self.eye = (step(self.eye.0, target.0), step(self.eye.1, target.1));
        }
        self.comp.set_eye(self.eye);
        let presents_before = self.present_ms();
        let start = Instant::now();
        let prompt = self.bs.prompt()?;
        let viewport = self.viewport;
        self.comp.notice =
            self.bs.notice.as_ref().map(|(n, _)| (n.heading.clone(), n.what.clone(), n.then.clone())).or_else(|| self.sound_notice.as_ref().map(|(n, _)| n.clone()));
        self.comp.draw(&mut self.bs.system, viewport, prompt.as_ref().map(|(p, n, t)| (p, *n, t.clone())))?;
        let compose = start.elapsed().as_secs_f64() * 1000.0;
        // R15: a new size of the surface is recorded, and the frames just
        // after it are excused.
        self.bs.surface(viewport);
        let drivers: Vec<f64> = self.present_ms().iter().zip(&presents_before).map(|(a, b)| a - b).collect();
        let start = Instant::now();
        if let Some(surface) = &mut self.surface {
            let mut buffer = surface.buffer_mut().map_err(|e| e.to_string())?;
            self.buffer_wait = start.elapsed().as_secs_f64() * 1000.0;
            let frame = self.comp.frame();
            let size = (frame.area.width() as u32, frame.area.height() as u32);
            // The damage, as at most a few rectangles: many small ones (a
            // tile row each) cost the window system more than they save.
            let mut damage = self.comp.damage().to_vec();
            if damage.len() > 4 {
                let b = damage.iter().fold(damage[0], |a, r| terse_host::soft::base::IRect::new(a.x0.min(r.x0), a.y0.min(r.y0), a.x1.max(r.x1), a.y1.max(r.y1)));
                damage = vec![b];
            }
            // Only what changed is copied and presented: the buffer holds
            // the frame of `age` presents ago (softbuffer), so the damage
            // since then. A new size, or an older buffer, is copied whole.
            let age = buffer.age() as usize;
            let known = age >= 1 && age <= self.presented.len() + 1 && self.presented.iter().take(age - 1).all(|p| p.0 == size) && buffer.len() == frame.data.len();
            let whole = [terse_host::soft::base::IRect::of_size(size)];
            let mut copy: Vec<terse_host::soft::base::IRect> = damage.clone();
            if known {
                for p in self.presented.iter().take(age - 1) {
                    copy.extend_from_slice(&p.1);
                }
            } else {
                copy = whole.to_vec();
            }
            let w = size.0 as usize;
            for r in &copy {
                let r = r.intersect(&whole[0]);
                for y in r.y0..r.y1 {
                    let row = y as usize * w;
                    buffer[row + r.x0 as usize..row + r.x1 as usize].copy_from_slice(&frame.data[row + r.x0 as usize..row + r.x1 as usize]);
                }
            }
            self.presented.push_front((size, damage.clone()));
            self.presented.truncate(2);
            let rects: Vec<softbuffer::Rect> = (if known { &damage[..] } else { &whole[..] })
                .iter()
                .filter_map(|r| {
                    let r = r.intersect(&whole[0]);
                    Some(softbuffer::Rect { x: r.x0 as u32, y: r.y0 as u32, width: NonZeroU32::new(r.width() as u32)?, height: NonZeroU32::new(r.height() as u32)? })
                })
                .collect();
            if rects.is_empty() {
                // Nothing changed: the same picture again (a commit keeps
                // the frame callbacks going), one pixel's damage.
                buffer.present_with_damage(&[softbuffer::Rect { x: 0, y: 0, width: NonZeroU32::MIN, height: NonZeroU32::MIN }]).map_err(|e| e.to_string())?;
            } else {
                buffer.present_with_damage(&rects).map_err(|e| e.to_string())?;
            }
        }
        let present_took = start.elapsed();
        let present = present_took.as_secs_f64() * 1000.0;
        // PERFORMANCE.md R15 step 1: the phosphor look is shed first when
        // frames run long to draw and present (terse-host's governor): the
        // core's tube, and a driver's own look through core.shed. The wait
        // for the window system to give a buffer back is not the host's
        // drawing: shedding would not shorten it.
        let drawing = compose + (present - self.buffer_wait).max(0.0);
        if !self.comp.looks.look.no_shed
            && let Some(floor) = self.governor.presented(Duration::from_secs_f64(drawing / 1000.0))
        {
            let why = if floor > self.comp.looks.look.floor { "late" } else { "recover" };
            self.comp.looks.look.floor = floor;
            if let Some(m) = &self.bs.monitor {
                m.add(terse_perflog::record::Line::Shed { frame: m.frame, level: floor as u32, why: why.into() });
            }
            // Said when it sheds further than before in the run (the
            // record has every change).
            if floor > self.shed_said {
                self.shed_said = floor;
                eprintln!("terse-bus: frames ran long: the phosphor look is shed to level {floor} of 3 while they do");
            }
        }
        let wait = Duration::from_secs_f64(self.buffer_wait.min(present) / 1000.0);
        self.bs.display_wait(wait);
        for line in self.bs.end_frame(present_took.saturating_sub(wait))? {
            eprintln!("{line}");
        }
        self.presents.push(Instant::now());
        if self.first_frame.is_none() {
            self.first_frame = Some(self.opts.startup + self.created.elapsed());
        }
        self.log.push(FrameLog { emulate, bus, compose, drivers, present, buffer_wait: self.buffer_wait, profile: self.comp.profile });
        self.frames += 1;
        Ok(())
    }

    /// A held frame period: no machine runs and nothing is drawn; the last
    /// picture is shown scaled to the surface (nearest pixel).
    fn hold(&mut self) -> Result<(), String> {
        let Some(surface) = &mut self.surface else { return Ok(()) };
        let mut buffer = surface.buffer_mut().map_err(|e| e.to_string())?;
        let frame = self.comp.frame();
        let (fw, fh) = (frame.area.width().max(0) as usize, frame.area.height().max(0) as usize);
        let (w, h) = (self.viewport.0 as usize, self.viewport.1 as usize);
        if buffer.len() < w * h {
            return Ok(());
        }
        terse_host::hold::scale_nearest(&frame.data, (fw, fh), &mut buffer[..w * h], (w, h));
        // The next frame drawn is copied whole.
        self.presented.clear();
        buffer.present().map_err(|e| e.to_string())
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        let size_now = (size.width.max(1), size.height.max(1));
        if size_now != self.viewport && self.frames > 0 {
            self.hold.resized(self.ticks);
        }
        self.viewport = size_now;
        if let Some(s) = &mut self.surface {
            let _ = s.resize(NonZeroU32::new(self.viewport.0).unwrap(), NonZeroU32::new(self.viewport.1).unwrap());
        }
    }

    fn key(&mut self, event: KeyEvent) {
        if self.opts.selftest.is_some() {
            return;
        }
        let pressed = event.state == ElementState::Pressed;
        let usage = match event.physical_key {
            PhysicalKey::Code(code) => hid_usage(code),
            _ => None,
        };
        // SPEC §machines.input: the home key is the host's, never a key event.
        if self.bs.system.is_system() && event.physical_key == PhysicalKey::Code(HOME_KEY) {
            if pressed && !event.repeat {
                self.pending.push(Event::home());
            }
            return;
        }
        // The volume: the host's keys, as in terse-host.
        if let PhysicalKey::Code(code) = event.physical_key
            && let Some(k) = terse_host::keymap::volume_key(code, self.modifiers)
        {
            if pressed {
                self.bs.swallow(usage);
                let what = self.audio.change(k);
                let then = "Ctrl+Alt+minus and plus change it, Ctrl+Alt+M mutes, for this run.".to_string();
                self.sound_notice = Some((("sound".into(), what, then), self.frames + 90));
            }
            return;
        }
        // Paste: Ctrl+V or Shift+Insert gives the clipboard's text as text events.
        let m = self.modifiers;
        let ctrl_v = m.control_key() && !m.alt_key() && !m.super_key() && event.physical_key == PhysicalKey::Code(KeyCode::KeyV);
        let shift_insert = m.shift_key() && event.physical_key == PhysicalKey::Code(KeyCode::Insert);
        if pressed && (ctrl_v || shift_insert) {
            self.bs.swallow(usage);
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

    fn paste_clipboard(&mut self) {
        if self.clipboard.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => self.clipboard = Some(c),
                Err(e) => return eprintln!("terse-bus: clipboard: {e}"),
            }
        }
        match self.clipboard.as_mut().unwrap().get_text() {
            Ok(text) => {
                let room = MAX_PASTE.saturating_sub(self.paste.len());
                self.paste.extend(paste_chars(&text).take(room));
            }
            Err(e) => eprintln!("terse-bus: clipboard: {e}"),
        }
    }

    /// Pointer movement and wheel steps gathered since the last frame, as
    /// at most one event each.
    fn coalesced_input(&mut self) {
        if self.opts.selftest.is_some() {
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
        self.pointer = Some(screen_pos(self.comp.shell_rect(self.viewport), pos.x, pos.y));
    }

    fn mouse_button(&mut self, state: ElementState, button: MouseButton) {
        if self.opts.selftest.is_some() {
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
        if let Some((x, y)) = self.pointer {
            self.pending.push(Event::pointer(x, y, self.buttons));
            self.pointer_sent = self.pointer;
        }
    }

    fn wheel(&mut self, delta: MouseScrollDelta) {
        let (x, y) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x as f64, y as f64),
            MouseScrollDelta::PixelDelta(p) => {
                let row = (self.comp.shell_rect(self.viewport).3 as f64 / terse_core::ROWS as f64).max(1.0);
                (p.x / row, p.y / row)
            }
        };
        self.wheel.0 -= y;
        self.wheel.1 -= x;
    }

    /// The selftest's report.
    fn report(&self) {
        let col = |f: fn(&FrameLog) -> f64| -> (f64, f64, f64) { spread(&mut self.log.iter().map(f).collect::<Vec<_>>()) };
        let (e50, e99, emax) = col(|l| l.emulate);
        let (b50, b99, bmax) = col(|l| l.bus);
        let (c50, c99, cmax) = col(|l| l.compose);
        let (p50, p99, pmax) = col(|l| l.present);
        let mut intervals: Vec<f64> = self.presents.windows(2).map(|w| (w[1] - w[0]).as_secs_f64() * 1000.0).collect();
        let (i50, i99, imax) = spread(&mut intervals);
        println!("selftest: {} frames, {}x{}", self.log.len(), self.viewport.0, self.viewport.1);
        println!("selftest: emulation incl. bus p50 {e50:.3} ms, p99 {e99:.3} ms, max {emax:.3} ms");
        println!("selftest: bus waits (summed over drivers) p50 {b50:.3} ms, p99 {b99:.3} ms, max {bmax:.3} ms");
        println!("selftest: composition p50 {c50:.3} ms, p99 {c99:.3} ms, max {cmax:.3} ms");
        {
            let bus = self.bs.bus.borrow();
            for (i, d) in bus.procs.iter().enumerate() {
                let (a, b, c) = spread(&mut self.log.iter().map(|l| l.drivers.get(i).copied().unwrap_or(0.0)).collect::<Vec<_>>());
                if c > 0.0 {
                    println!("selftest:   PRESENT to slots {:?} ({}) p50 {a:.3} ms, p99 {b:.3} ms, max {c:.3} ms", d.slots, d.info.name);
                }
            }
        }
        // The drivers draw at once: the core's own is what is left of the
        // composition after the PRESENT round (sent to all, then collected).
        let (o50, o99, omax) = col(|l| l.compose - l.profile.present_ns as f64 / 1e6);
        let (r50, r99, rmax) = col(|l| l.profile.present_ns as f64 / 1e6);
        println!("selftest:   the PRESENT round (all drivers) p50 {r50:.3} ms, p99 {r99:.3} ms, max {rmax:.3} ms");
        println!("selftest:   the core's own composition p50 {o50:.3} ms, p99 {o99:.3} ms, max {omax:.3} ms");
        for (what, f) in [
            ("properties and styles", (|l: &FrameLog| l.profile.show_ns) as fn(&FrameLog) -> u64),
            ("layers composed (or the tube)", |l| l.profile.layers_ns),
            ("the frame put together", |l| l.profile.frame_ns),
        ] {
            let (a, b, c) = spread(&mut self.log.iter().map(|l| f(l) as f64 / 1e6).collect::<Vec<_>>());
            println!("selftest:     {what} p50 {a:.3} ms, p99 {b:.3} ms, max {c:.3} ms");
        }
        println!("selftest: present (copy) p50 {p50:.3} ms, p99 {p99:.3} ms, max {pmax:.3} ms");
        let (w50, w99, wmax) = col(|l| l.buffer_wait);
        println!("selftest:   waiting for the window system to release a buffer p50 {w50:.3} ms, p99 {w99:.3} ms, max {wmax:.3} ms");
        println!("selftest: present interval p50 {i50:.3} ms, p99 {i99:.3} ms, max {imax:.3} ms");
        let (t50, t99, tmax) = col(|l| l.emulate + l.compose + l.present - l.buffer_wait.min(l.present));
        println!("selftest: frame work (emulation + composition + present, less the window system's wait) p50 {t50:.3} ms, p99 {t99:.3} ms, max {tmax:.3} ms");
        if let Some(m) = &self.bs.monitor {
            println!("selftest: late frames {} (the record has who)", m.late);
        }
        println!("selftest: frame periods held while the window was resized {}", self.hold.held);
        if let Some(f) = self.first_frame {
            println!("selftest: start-up to first frame {:.1} ms", f.as_secs_f64() * 1000.0);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("terse-bus").with_inner_size(PhysicalSize::new(self.viewport.0, self.viewport.1));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.error = Some(e.to_string());
                event_loop.exit();
                return;
            }
        };
        let surface = softbuffer::Context::new(window.clone()).and_then(|c| softbuffer::Surface::new(&c, window.clone()));
        match surface {
            Ok(s) => self.surface = Some(s),
            Err(e) => {
                self.error = Some(format!("softbuffer: {e}"));
                event_loop.exit();
                return;
            }
        }
        let size = window.inner_size();
        self.window = Some(window);
        self.resize(size);
        self.next = Instant::now();
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.resize(size),
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
        if self.window.is_none() {
            return;
        }
        if signals::stop_requested() {
            event_loop.exit();
            return;
        }
        let now = Instant::now();
        if now >= self.next {
            while let Some(&(f, (w, h))) = self.resizes.front()
                && f <= self.ticks
            {
                self.resizes.pop_front();
                // As a configure from the window system would: the surface
                // takes the size (a tiling compositor sets it; a kiosk or
                // fullscreen one shows a smaller buffer centred).
                self.resize(PhysicalSize::new(w, h));
            }
            // The window's size as the window system has it now (a size
            // whose Resized never came is taken here), as terse-host does.
            if self.opts.selftest.is_none()
                && let Some(w) = self.window.clone()
            {
                let s = w.inner_size();
                if s.width > 0 && s.height > 0 && (s.width, s.height) != self.viewport {
                    self.resize(s);
                }
            }
            self.coalesced_input();
            self.ticks += 1;
            // R15, resizing: while the size changes the machines are held
            // (bounded: MAX_HOLD in a row, then RUN_BETWEEN frames run).
            if self.hold.step(self.ticks) {
                if let Err(e) = self.hold() {
                    self.error = Some(e);
                    event_loop.exit();
                    return;
                }
            } else {
                if self.bs.system.stopped().is_none()
                    && let Err(e) = self.frame()
                {
                    self.error = Some(e);
                    event_loop.exit();
                    return;
                }
            }
            if self.opts.selftest.is_some_and(|n| self.frames >= n) || (self.opts.selftest.is_some() && self.bs.system.stopped().is_some()) {
                event_loop.exit();
                return;
            }
            // Virtual time lags rather than catching up (SPEC §frames).
            self.next = (self.next + FRAME).max(now);
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next));
    }
}

/// Runs the system in a window until it stops (a single machine), the
/// window is closed, a signal comes, or the selftest's frames are done.
pub fn run(bs: BusSystem, script: Script, native: (u32, u32), opts: Options) -> Result<ExitCode, String> {
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let mut comp = Compositor::new(bs.system.shell().devices(), native, opts.threads);
    comp.looks = opts.looks;
    if opts.flat {
        comp.panel_mode = terse_host::soft::panel::PanelMode::Flat;
    }
    let viewport = opts.size.unwrap_or(native);
    let eye = opts.eye;
    let opts_selftest = opts.selftest.is_some();
    let mut app = App {
        bs,
        script,
        comp,
        opts,
        window: None,
        surface: None,
        viewport,
        pending: Vec::new(),
        pointer: None,
        pointer_sent: None,
        pointer_px: None,
        buttons: 0,
        wheel: (0.0, 0.0),
        modifiers: ModifiersState::empty(),
        paste: VecDeque::new(),
        clipboard: None,
        eye,
        next: Instant::now(),
        error: None,
        frames: 0,
        log: Vec::new(),
        presents: Vec::new(),
        first_frame: None,
        created: Instant::now(),
        governor: Default::default(),
        presented: VecDeque::new(),
        buffer_wait: 0.0,
        shed_said: 0,
        resizes: VecDeque::new(),
        ticks: 0,
        hold: terse_host::hold::ResizeHold::default(),
        audio: terse_host::audio::Audio::new(terse_host::audio::Choice::from_env(opts_selftest)),
        sound_notice: None,
    };
    app.resizes = app.opts.resizes.iter().copied().collect();
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    for line in app.bs.own_late() {
        eprintln!("{line}");
    }
    if app.opts.selftest.is_some() {
        app.report();
        // The hashes, to compare with a headless run of the same script.
        if let Ok((shell, children)) = app.bs.hashes() {
            println!("{}", hex(&shell));
            for (h, x) in children {
                println!("{h} {}", x.map_or("-".to_string(), |x| hex(&x)));
            }
        }
    }
    let code = match app.bs.system.stopped() {
        Some(terse_core::Stopped::Exit(c)) => ExitCode::from(c),
        Some(terse_core::Stopped::Fault(_)) => ExitCode::from(255),
        Some(terse_core::Stopped::Halted(slot)) => {
            let saved = app.bs.halted.as_ref().map_or(String::new(), |(_, d)| format!("; its state before the stop: {d}"));
            eprintln!("terse-bus: stopped by the host: device {slot} did not keep up{saved}");
            ExitCode::from(4)
        }
        None => ExitCode::SUCCESS,
    };
    match app.error.take() {
        Some(e) => Err(e),
        None => Ok(code),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
