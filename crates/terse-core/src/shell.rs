//! A system of machines (SPEC 12): the shell, which has device 8, and the
//! machines it starts. The system decides the order of frames, routes the
//! host's input and keeps device 8's records up to date; a [`Driver`]
//! (headless or windowed) supplies everything else a machine's frame needs
//! (content completions, page events, random bytes) and fetches images.
//!
//! Without device 8 (a plain image, or a shell image not run as the shell)
//! the system is a single machine: every event goes to it unchanged and the
//! home key is dropped.

use crate::image::Image;
use crate::machine::{FrameResult, Machine, Stopped};
use crate::machines::{MAX_MACHINES, ProgramId, StartFailure, map_into, picture, state};
use crate::system::{CONTROLLERS, Controller, Event};

/// Which machine a driver is asked to run: handle 0 is the shell; `serial`
/// tells apart machines that had the same handle (the shell's is 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Who {
    pub handle: u8,
    pub serial: u32,
}

/// What a host does for the machines of a system.
pub trait Driver {
    /// Runs one frame of `machine` with the input `events` the system
    /// routed to it (the driver adds the device events it produces itself:
    /// content completions, page events, feeds updated) and `controllers`.
    fn frame(&mut self, who: Who, machine: &mut Machine, events: Vec<Event>, controllers: [Controller; CONTROLLERS])
    -> FrameResult;
    /// The shell started machine `serial`: fetch the image with this ID
    /// and hand it to [`System::deliver_image`].
    fn want_image(&mut self, serial: u32, image: [u8; 32]);
    /// The shell started machine `serial` as a program (`start_program`):
    /// find the current version the host holds (a built-in by its title,
    /// among `builtins`, for the all-zero key), fetch it and hand it to
    /// [`System::deliver_image`].
    fn want_program(&mut self, serial: u32, program: &ProgramId, builtins: &[[u8; 32]]);
    /// Machine `serial` ceased to exist (the shell stopped it).
    fn removed(&mut self, serial: u32);
}

/// A machine the shell started.
pub struct Child {
    pub serial: u32,
    /// The image ID given to `start`; for `start_program`, that of the
    /// version the host delivered (zero until then).
    pub image: [u8; 32],
    /// The identity it was started with (`start_program`).
    pub identity: Option<ProgramId>,
    /// The machine, once its image has been loaded.
    pub machine: Option<Machine>,
    /// The image's title, once loaded.
    pub title: String,
    /// Why the start failed, if it did.
    pub error: Option<String>,
    /// The image, delivered but not yet latched (SPEC 12.5).
    delivered: Option<Result<Image, StartFailure>>,
    /// Units used in its last frame.
    last_used: u32,
}

/// What one system frame did.
#[derive(Clone, Debug, Default)]
pub struct SystemFrame {
    /// The shell's frame, unless the shell had already stopped.
    pub shell: Option<FrameResult>,
    /// The children that ran, by handle.
    pub children: Vec<(u8, FrameResult)>,
}

pub struct System {
    shell: Machine,
    children: [Option<Child>; MAX_MACHINES],
    next_serial: u32,
    /// Buttons of the last pointer event (to tell presses).
    buttons: u8,
}

const NO_CONTROLLERS: [Controller; CONTROLLERS] = [Controller { connected: false, buttons: 0, left: (0, 0), right: (0, 0) }; CONTROLLERS];

impl System {
    /// A system whose shell runs `image`. With `shell`, the image is the
    /// shell: it gets device 8 if it lists it, and `builtins` are the IDs of
    /// the host's built-in programs. Without, it is a single machine.
    pub fn new(image: &Image, shell: bool, builtins: Vec<[u8; 32]>) -> Self {
        let shell = if shell { Machine::new_shell(image, builtins) } else { Machine::new(image) };
        System { shell, children: Default::default(), next_serial: 1, buttons: 0 }
    }

    pub fn shell(&self) -> &Machine {
        &self.shell
    }

    pub fn shell_mut(&mut self) -> &mut Machine {
        &mut self.shell
    }

    /// The shell's identity (a built-in shell's, SPEC 12.2), whose named
    /// records it reads (SPEC 9.6).
    pub fn set_shell_identity(&mut self, identity: Option<ProgramId>) {
        self.shell.set_identity(identity);
    }

    /// Whether the shell has device 8: this is a system, not a single machine.
    pub fn is_system(&self) -> bool {
        self.shell.machines().is_some()
    }

    /// The machine with handle `h` (1-8), if the shell started one.
    pub fn child(&self, h: u8) -> Option<&Child> {
        self.children.get(h.wrapping_sub(1) as usize)?.as_ref()
    }

    /// The children, with their handles, in increasing handle order.
    pub fn children(&self) -> impl Iterator<Item = (u8, &Child)> {
        self.children.iter().enumerate().filter_map(|(i, c)| Some((i as u8 + 1, c.as_ref()?)))
    }

    /// The machine with this serial (0: the shell).
    pub fn machine_by_serial(&self, serial: u32) -> Option<(u8, &Machine)> {
        if serial == 0 {
            return Some((0, &self.shell));
        }
        self.children().find(|(_, c)| c.serial == serial).and_then(|(h, c)| Some((h, c.machine.as_ref()?)))
    }

    /// Handle `h`'s state as device 8 records it (`state::NONE` if none).
    pub fn state(&self, h: u8) -> u8 {
        self.shell.machines().map_or(state::NONE, |m| m.slot(h).state)
    }

    /// The machine that has the focus (0: the shell).
    pub fn focus(&self) -> u8 {
        self.shell.machines().map_or(0, |m| m.focus())
    }

    /// The children's pictures as the host shows them: handle, rectangle
    /// of the picture in the shell's units, in drawing order (SPEC 12.3).
    pub fn pictures(&self) -> Vec<(u8, crate::page::Region)> {
        let Some(dev) = self.shell.machines() else { return Vec::new() };
        self.children()
            .filter(|(h, c)| c.machine.is_some() && dev.slot(*h).has_picture())
            .filter_map(|(h, _)| Some((h, picture(dev.slot(h).rect)?)))
            .collect()
    }

    /// The image for machine `serial` arrived, or could not be had; it is
    /// taken into account at the start of the next frame (SPEC 12.5).
    pub fn deliver_image(&mut self, serial: u32, image: Result<Image, StartFailure>) {
        if let Some(c) = self.children.iter_mut().flatten().find(|c| c.serial == serial && c.machine.is_none()) {
            c.delivered = Some(image);
        }
    }

    /// Whether the system has stopped: the shell exited or faulted.
    pub fn stopped(&self) -> Option<Stopped> {
        self.shell.stopped()
    }

    /// Runs one frame of the system (SPEC 12.5): latches what changed,
    /// routes `events` (the host's input, in the shell's screen units), runs
    /// the shell, carries out its starts and stops, then runs each running
    /// child in increasing handle order. `controllers` go to the machine
    /// with the focus.
    pub fn frame(&mut self, events: &[Event], controllers: [Controller; CONTROLLERS], driver: &mut dyn Driver) -> SystemFrame {
        let mut out = SystemFrame::default();
        if self.shell.stopped().is_some() {
            return out;
        }
        let mut shell_events = self.latch();
        let mut routed: [Vec<Event>; MAX_MACHINES] = Default::default();
        self.route(events, &mut shell_events, &mut routed);

        // The shell.
        let focus = self.focus();
        let shell_controllers = if focus == 0 { controllers } else { NO_CONTROLLERS };
        out.shell = Some(driver.frame(Who { handle: 0, serial: 0 }, &mut self.shell, shell_events, shell_controllers));

        // Its starts and stops, in order.
        if let Some(dev) = self.shell.machines_mut() {
            let builtins = dev.builtins().to_vec();
            for command in dev.take_commands() {
                match command {
                    crate::machines::Command::Start { handle, .. } | crate::machines::Command::StartProgram { handle, .. } => {
                        let serial = self.next_serial;
                        self.next_serial += 1;
                        let i = handle as usize - 1;
                        if let Some(old) = self.children[i].take() {
                            driver.removed(old.serial);
                        }
                        let (image, identity) = match &command {
                            crate::machines::Command::Start { image, .. } => (*image, None),
                            crate::machines::Command::StartProgram { program, .. } => ([0; 32], Some(program.clone())),
                            _ => unreachable!(),
                        };
                        self.children[i] = Some(Child {
                            serial,
                            image,
                            identity: identity.clone(),
                            machine: None,
                            title: String::new(),
                            error: None,
                            delivered: None,
                            last_used: 0,
                        });
                        match &identity {
                            None => driver.want_image(serial, image),
                            Some(program) => driver.want_program(serial, program, &builtins),
                        }
                    }
                    crate::machines::Command::Stop { handle } => {
                        if let Some(old) = self.children[handle as usize - 1].take() {
                            driver.removed(old.serial);
                        }
                    }
                }
            }
        }

        // The children.
        let focus = self.focus();
        for i in 0..MAX_MACHINES {
            let h = i as u8 + 1;
            if self.state(h) != state::RUNNING {
                continue;
            }
            let Some(child) = self.children[i].as_mut() else { continue };
            let Some(machine) = child.machine.as_mut().filter(|m| m.stopped().is_none()) else { continue };
            let pads = if focus == h { controllers } else { NO_CONTROLLERS };
            let events = std::mem::take(&mut routed[i]);
            let result = driver.frame(Who { handle: h, serial: child.serial }, machine, events, pads);
            child.last_used = result.used.min(u32::MAX as u64) as u32;
            out.children.push((h, result));
        }
        out
    }

    /// Step 1 (SPEC 12.5): images delivered and machines that stopped
    /// become the handles' new states, and each record takes the machine's
    /// latest numbers. Returns the machine-changed events for the shell.
    fn latch(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        let Some(dev) = self.shell.machines_mut() else { return events };
        for (i, slot) in self.children.iter_mut().enumerate() {
            let h = i as u8 + 1;
            let Some(child) = slot.as_mut() else { continue };
            let (record_state, run) = (dev.slot(h).state, dev.slot(h).run);
            let mut new = None;
            match record_state {
                state::LOADING => match child.delivered.take() {
                    Some(Ok(image)) => {
                        child.title = image.title_lossy();
                        child.image = image.id;
                        let mut machine = Machine::new(&image);
                        machine.set_identity(child.identity.clone());
                        child.machine = Some(machine);
                        dev.slot_mut(h).image = image.id;
                        new = Some(if run { state::RUNNING } else { state::PAUSED });
                    }
                    Some(Err(f)) => {
                        child.error = Some(f.message);
                        let s = dev.slot_mut(h);
                        s.failure = f.reason;
                        if let Some(id) = f.image {
                            child.image = id;
                            s.image = id;
                        }
                        new = Some(state::FAILED);
                    }
                    None => {}
                },
                state::RUNNING => {
                    let s = dev.slot_mut(h);
                    match child.machine.as_ref().and_then(Machine::stopped) {
                        Some(Stopped::Exit(code)) => {
                            s.exit_code = code;
                            new = Some(state::EXITED);
                        }
                        Some(Stopped::Fault(f)) => {
                            s.fault_kind = crate::machine::fault_number(&f);
                            s.fault_pc = f.pc;
                            new = Some(state::FAULTED);
                        }
                        None => {}
                    }
                }
                _ => {}
            }
            if let Some(state) = new {
                dev.set_state(h, state);
                events.push(Event::machine_changed(h, state));
            }
            if let Some(m) = &child.machine {
                let s = dev.slot_mut(h);
                s.units = child.last_used;
                s.frames = m.frame();
            }
        }
        events
    }

    /// Step 2 (SPEC 12.4): each input event goes to the shell or to one
    /// child, by the focus and the pictures as they are now.
    fn route(&mut self, events: &[Event], shell: &mut Vec<Event>, routed: &mut [Vec<Event>; MAX_MACHINES]) {
        if !self.is_system() {
            shell.extend(events.iter().filter(|e| e.kind != Event::HOME));
            return;
        }
        let pictures = self.pictures();
        for &e in events {
            match e.kind {
                Event::HOME => {
                    self.shell.machines_mut().unwrap().home();
                    shell.push(e);
                }
                Event::POINTER => {
                    let (x, y, buttons) = e.pointer_parts();
                    let pressed = buttons & !self.buttons != 0;
                    self.buttons = buttons;
                    // The topmost picture under the pointer: the highest handle.
                    let hit = pictures.iter().rev().find_map(|&(h, p)| Some((h, map_into(p, x, y)?)));
                    match hit {
                        None => shell.push(e),
                        Some((h, (cx, cy))) => {
                            if pressed {
                                shell.push(Event::machine_pressed(h, buttons));
                            }
                            if self.state(h) == state::RUNNING {
                                routed[h as usize - 1].push(Event::pointer(cx, cy, buttons));
                            }
                        }
                    }
                }
                Event::KEY_DOWN | Event::KEY_UP | Event::TEXT | Event::WHEEL => match self.focus() {
                    0 => shell.push(e),
                    h if self.state(h) == state::RUNNING => routed[h as usize - 1].push(e),
                    _ => {}
                },
                _ => shell.push(e),
            }
        }
    }
}
