//! The composed picture in a window (softbuffer), and the window's input
//! as the machine's events (SPEC 5). Implemented for completeness; the
//! prototype's tests and measurements use `--screenshot` and `--bench`.

use std::num::NonZeroU32;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use terse_bus::bus::Bus;
use terse_bus::compose::{Compositor, screen_rect};
use terse_core::{Controller, Event, FrameInput, Machine, Script};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

const FRAME: Duration = Duration::from_micros(16_667);

struct App {
    core: Machine<Bus>,
    script: Script,
    native: (u32, u32),
    comp: Compositor,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    events: Vec<Event>,
    pointer: (u16, u16),
    buttons: u8,
    next: Instant,
    error: Option<String>,
}

/// USB HID usage IDs of the keys the prototype maps (SPEC 5).
fn hid(key: KeyCode) -> Option<u8> {
    use KeyCode::*;
    let letters = [KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ];
    if let Some(i) = letters.iter().position(|&k| k == key) {
        return Some(0x04 + i as u8);
    }
    let digits = [Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9, Digit0];
    if let Some(i) = digits.iter().position(|&k| k == key) {
        return Some(0x1E + i as u8);
    }
    Some(match key {
        Enter => 0x28,
        Escape => 0x29,
        Backspace => 0x2A,
        Tab => 0x2B,
        Space => 0x2C,
        PageUp => 0x4B,
        PageDown => 0x4E,
        Home => 0x4A,
        End => 0x4D,
        ArrowRight => 0x4F,
        ArrowLeft => 0x50,
        ArrowDown => 0x51,
        ArrowUp => 0x52,
        _ => return None,
    })
}

impl App {
    fn frame(&mut self) -> Result<(), String> {
        let frame = self.core.frame();
        let mut events = self.script.events(frame).to_vec();
        events.append(&mut self.events);
        let controllers = [Controller::default(); terse_core::CONTROLLERS];
        self.core.try_run_frame(&FrameInput { events: &events, controllers, ..Default::default() })?;
        self.core.devices_mut().flush()?;
        let rect = screen_rect(self.native, self.comp.size);
        let updates = self.core.devices_mut().present(self.comp.size, rect)?;
        self.comp.update(&updates)?;
        let Some(surface) = &mut self.surface else { return Ok(()) };
        let mut buffer = surface.buffer_mut().map_err(|e| e.to_string())?;
        let n = buffer.len().min(self.comp.frame.len());
        buffer[..n].copy_from_slice(&self.comp.frame[..n]);
        buffer.present().map_err(|e| e.to_string())
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        let (w, h) = (size.width.max(1), size.height.max(1));
        let layers = self.core.devices_mut().layers().into_iter().map(|(_, s, _, l)| (s, l.id)).collect();
        self.comp = Compositor::new((w, h), layers);
        if let Some(s) = &mut self.surface {
            let _ = s.resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap());
        }
    }

    fn pointer_event(&mut self) {
        self.events.push(Event::pointer(self.pointer.0, self.pointer.1, self.buttons));
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("terse-bus").with_inner_size(PhysicalSize::new(self.comp.size.0, self.comp.size.1));
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
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key
                    && let Some(k) = hid(code)
                {
                    let down = event.state == ElementState::Pressed;
                    self.events.push(if down { Event::key_down(k, 0) } else { Event::key_up(k, 0) });
                }
                if event.state == ElementState::Pressed
                    && let Some(text) = &event.text
                {
                    self.events.extend(text.chars().filter(|c| !c.is_control()).map(Event::text));
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y, w, h) = screen_rect(self.native, self.comp.size);
                let ux = (((position.x as f32 - x) / w * 640.0).floor()).clamp(0.0, 639.0) as u16;
                let uy = (((position.y as f32 - y) / h * 480.0).floor()).clamp(0.0, 479.0) as u16;
                if (ux, uy) != self.pointer {
                    self.pointer = (ux, uy);
                    self.pointer_event();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let bit = match button {
                    MouseButton::Left => 1,
                    MouseButton::Right => 2,
                    MouseButton::Middle => 4,
                    _ => 0,
                };
                if state == ElementState::Pressed { self.buttons |= bit } else { self.buttons &= !bit }
                self.pointer_event();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y,
                    MouseScrollDelta::PixelDelta(p) => -(p.y as f32) / 40.0,
                };
                let v = steps.round().clamp(-127.0, 127.0) as i8;
                if v != 0 {
                    self.events.push(Event::wheel(v, 0));
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if now >= self.next {
            if let Err(e) = self.frame() {
                self.error = Some(e);
                event_loop.exit();
                return;
            }
            if self.core.stopped().is_some() {
                event_loop.exit();
                return;
            }
            // Virtual time lags rather than catching up (SPEC 1.4).
            self.next = (self.next + FRAME).max(now);
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next));
    }
}

/// Runs the machine in a window until it stops or the window is closed.
pub fn run(core: Machine<Bus>, script: Script, native: (u32, u32), comp: Compositor) -> Result<ExitCode, String> {
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let mut app = App {
        core,
        script,
        native,
        comp,
        window: None,
        surface: None,
        events: Vec::new(),
        pointer: (0, 0),
        buttons: 0,
        next: Instant::now(),
        error: None,
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    app.core.devices_mut().shutdown();
    match app.error {
        Some(e) => Err(e),
        None => Ok(ExitCode::SUCCESS),
    }
}
