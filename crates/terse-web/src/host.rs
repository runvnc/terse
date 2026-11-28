//! The page's host: a system (SPEC §machines) of the built-in shell and the
//! machines it starts, or one program alone, run one frame at a time as
//! the page asks, with the input the page gives.
//!
//! As `terse run` and `terse-host --screenshot` run them (terse-net's
//! `HeadlessSystem`): every content request and every image a shell starts
//! is finished before the next frame, and each page (device 7) is laid out
//! between frames with the reference layout (SPEC §page.reference), so its
//! height arrives in the frame after a change. Unlike those, the clock is
//! the real one and the random bytes are fresh, as in a window.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use terse_core::page::{Layout, Layouter, PageInput, reference_layout};
use terse_core::{
    CONTROLLERS, Completion, Controller, Driver, Event, FrameInput, FrameResult, Image, Machine,
    ProgramId, StartFailure, System, Who, load_image,
};

use crate::bundle;
use crate::content::Content;

/// The latest layout of a machine's page, shared with the drawing.
pub type Shown = Arc<Mutex<Option<Arc<Layout>>>>;

/// The reference layout, keeping what it laid out for the drawing.
struct KeptLayouter {
    layout: Arc<Layout>,
    shown: Shown,
}

impl Layouter for KeptLayouter {
    fn layout(&mut self, doc: &[u8], width: u16) -> u32 {
        self.layout = Arc::new(reference_layout(doc, width));
        *self.shown.lock().unwrap() = Some(self.layout.clone());
        self.layout.height
    }

    fn anchor_at(&self, x: u32, y: u32) -> u16 {
        self.layout.anchor_at(x, y)
    }

    fn height(&self) -> u32 {
        self.layout.height
    }

    fn locate(&self, id: u16) -> Option<(u32, u32)> {
        self.layout.locate(id)
    }
}

/// What the host keeps for one machine.
struct MachineIo {
    page: PageInput,
    shown: Shown,
    completions: Vec<Completion>,
    /// Random bytes for its next frame; those a frame takes are replaced,
    /// so every byte a program sees is fresh.
    random: Vec<u8>,
}

/// Random bytes a machine may take in one frame. SPEC §system allows a
/// frame far more (`MAX_RANDOM_PER_FRAME`, megabytes); a call beyond these
/// gets zeros, which no program here comes near.
const RANDOM_PER_FRAME: usize = 1 << 16;

/// Fills `out` with random bytes: the browser's `crypto.getRandomValues`
/// in the page, zeros elsewhere (the tests, which want them reproducible).
pub fn fill_random(out: &mut [u8]) {
    #[cfg(target_arch = "wasm32")]
    {
        #[link(wasm_import_module = "env")]
        unsafe extern "C" {
            fn tw_random(ptr: *mut u8, len: usize);
        }
        unsafe { tw_random(out.as_mut_ptr(), out.len()) };
    }
    #[cfg(not(target_arch = "wasm32"))]
    out.fill(0);
}

/// The system's driver: each machine's input besides the routed events,
/// and its requests.
struct WebDriver<'a> {
    io: &'a mut HashMap<u32, MachineIo>,
    content: &'a mut Content,
    /// Images wanted this frame (serial, image ID or failure).
    images: &'a mut Vec<(u32, Result<Image, StartFailure>)>,
    unix_time: u32,
    copied: &'a mut Option<Vec<u8>>,
}

impl Driver for WebDriver<'_> {
    fn frame(&mut self, who: Who, machine: &mut Machine, events: Vec<Event>, controllers: [Controller; CONTROLLERS]) -> FrameResult {
        let io = self.io.entry(who.serial).or_insert_with(|| {
            let shown: Shown = Arc::default();
            let layouter = KeptLayouter { layout: Arc::default(), shown: shown.clone() };
            let mut random = vec![0; RANDOM_PER_FRAME];
            fill_random(&mut random);
            MachineIo { page: PageInput::new(Box::new(layouter)), shown, completions: Vec::new(), random }
        });
        let events = io.page.events(machine.page(), &events);
        let completions = std::mem::take(&mut io.completions);
        let input = FrameInput { events: &events, unix_time: self.unix_time, random: &io.random, completions: &completions, controllers };
        let result = machine.run_frame(&input);
        fill_random(&mut io.random[..result.random_used.min(RANDOM_PER_FRAME)]);
        io.page.after_frame(machine.page());
        for request in machine.take_content_requests() {
            let handle = request.handle;
            let outcome = self.content.answer(machine, request);
            io.completions.push(Completion { handle, outcome });
        }
        if let Some(t) = machine.take_copied() {
            *self.copied = Some(t);
        }
        result
    }

    fn want_image(&mut self, serial: u32, image: [u8; 32]) {
        let got = match self.content.get(&image) {
            Some(bytes) => load_image(bytes),
            None => Err(StartFailure::not_found("image not found")),
        };
        self.images.push((serial, got));
    }

    fn want_program(&mut self, serial: u32, program: &ProgramId, builtins: &[[u8; 32]]) {
        let found = program.is_builtin().then(|| self.content.find_builtin(&program.name, builtins)).flatten();
        match found {
            Some(id) => self.want_image(serial, id),
            None => self.images.push((serial, Err(StartFailure::not_found("program not found (no network in the page)")))),
        }
    }

    fn removed(&mut self, serial: u32) {
        self.io.remove(&serial);
        self.images.retain(|i| i.0 != serial);
    }
}

/// The page's input, as the window gathers it (terse-host `app.rs`):
/// keys, text and button changes in order, pointer movement and wheel
/// steps at most one event each per frame, pasted text as the queue has
/// room.
#[derive(Default)]
pub struct Input {
    pending: Vec<Event>,
    pointer: Option<(u16, u16)>,
    pointer_sent: Option<(u16, u16)>,
    buttons: u8,
    wheel: (f64, f64),
    paste: std::collections::VecDeque<char>,
    /// The paste key, held back until its text has gone (SPEC §events).
    paste_key: Option<(u8, u8, bool)>,
    home: bool,
}

/// Pasted characters given per frame (the queue holds 64 events).
const PASTE_PER_FRAME: usize = 32;

impl Input {
    pub fn key(&mut self, down: bool, usage: u8, mods: u8) {
        if !down
            && let Some((k, m, _)) = self.paste_key.filter(|p| p.0 == usage)
        {
            if self.paste.is_empty() {
                self.paste_key = None;
                self.pending.push(Event::key_up(k, m));
            } else {
                self.paste_key = Some((k, m, true));
            }
            return;
        }
        self.pending.push(if down { Event::key_down(usage, mods) } else { Event::key_up(usage, mods) });
    }

    pub fn text(&mut self, ch: char) {
        if !ch.is_control() {
            self.pending.push(Event::text(ch));
        }
    }

    /// The paste key `usage` was pressed and `text` is what it pastes.
    pub fn paste(&mut self, usage: u8, mods: u8, text: &str) {
        self.pending.push(Event::key_down(usage, mods));
        let mut prev = '\0';
        for c in text.chars() {
            let out = match c {
                '\r' => Some('\n'),
                '\n' if prev == '\r' => None,
                '\n' | '\t' => Some(c),
                c if c.is_control() => None,
                c => Some(c),
            };
            prev = c;
            if self.paste.len() < 1 << 20 {
                self.paste.extend(out);
            }
        }
        self.paste_key = Some((usage, mods, false));
    }

    pub fn pointer(&mut self, x: u16, y: u16) {
        self.pointer = Some((x.min(639), y.min(479)));
    }

    pub fn buttons(&mut self, buttons: u8) {
        self.buttons = buttons & 7;
        if let Some((x, y)) = self.pointer {
            self.pending.push(Event::pointer(x, y, self.buttons));
            self.pointer_sent = self.pointer;
        }
    }

    /// Wheel travel in steps, positive down and right.
    pub fn wheel(&mut self, v: f64, h: f64) {
        self.wheel.0 += v;
        self.wheel.1 += h;
    }

    pub fn home(&mut self) {
        self.home = true;
    }

    /// The events of the next frame.
    fn take(&mut self, system: bool) -> Vec<Event> {
        if std::mem::take(&mut self.home) && system {
            self.pending.push(Event::home());
        }
        if let Some(p) = self.pointer
            && self.pointer != self.pointer_sent
        {
            self.pending.push(Event::pointer(p.0, p.1, self.buttons));
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
        for _ in 0..PASTE_PER_FRAME {
            let Some(c) = self.paste.pop_front() else { break };
            self.pending.push(Event::text(c));
        }
        if self.paste.is_empty()
            && let Some((k, m, true)) = self.paste_key
        {
            self.paste_key = None;
            self.pending.push(Event::key_up(k, m));
        }
        std::mem::take(&mut self.pending)
    }
}

/// What the page asked to run.
pub enum Start<'a> {
    /// The built-in shell with every built-in program.
    Shell,
    /// The built-in program with this title, alone.
    Alone(&'a str),
}

pub struct Host {
    pub system: System,
    content: Content,
    io: HashMap<u32, MachineIo>,
    images: Vec<(u32, Result<Image, StartFailure>)>,
    pub input: Input,
    /// Text a machine offered the clipboard (SPEC §system `copy`).
    pub copied: Option<Vec<u8>>,
    /// Budget units each machine used in the last frame (handle, units).
    pub used: Vec<(u8, u64)>,
}

impl Host {
    /// A host for the content `bundle` (see [`bundle`]), running `start`.
    pub fn new(bundle_bytes: &[u8], start: Start) -> Result<Host, String> {
        let mut content = Content::default();
        let (mut shell, mut builtins, mut index) = (None, Vec::new(), None);
        for (kind, data) in bundle::decode(bundle_bytes)? {
            match kind {
                bundle::SHELL => shell = Some(data),
                bundle::PROGRAM => builtins.push(content.put(data)),
                bundle::MANUAL_INDEX => index = Some(content.put(data)),
                _ => {
                    content.put(data);
                }
            }
        }
        let (system, identity) = match start {
            Start::Shell => {
                let image = load_image(&shell.ok_or("the bundle has no shell")?).map_err(|e| e.message)?;
                let identity = ProgramId { publisher: [0; 32], name: image.title.clone() };
                (System::new(&image, true, builtins), identity)
            }
            Start::Alone(title) => {
                let id = content.find_builtin(title.as_bytes(), &builtins).ok_or(format!("no built-in program {title}"))?;
                let image = load_image(content.get(&id).unwrap()).map_err(|e| e.message)?;
                let identity = ProgramId { publisher: [0; 32], name: image.title.clone() };
                let mut system = System::new(&image, false, Vec::new());
                // The reader alone opens the manual (as M on its timeline would).
                if title == "reader" {
                    system.shell_mut().set_argument(index);
                }
                (system, identity)
            }
        };
        let mut system = system;
        system.set_shell_identity(Some(identity));
        Ok(Host {
            system,
            content,
            io: HashMap::new(),
            images: Vec::new(),
            input: Input::default(),
            copied: None,
            used: Vec::new(),
        })
    }

    /// Runs the system's next frame at `unix_time`; false once the system
    /// has stopped (the shell, or the machine alone, exited or faulted).
    pub fn frame(&mut self, unix_time: u32) -> bool {
        if self.system.stopped().is_some() {
            return false;
        }
        for (serial, image) in self.images.drain(..) {
            self.system.deliver_image(serial, image);
        }
        let events = self.input.take(self.system.is_system());
        let mut driver = WebDriver {
            io: &mut self.io,
            content: &mut self.content,
            images: &mut self.images,
            unix_time,
            copied: &mut self.copied,
        };
        let none = Controller { connected: false, buttons: 0, left: (0, 0), right: (0, 0) };
        let done = self.system.frame(&events, [none; CONTROLLERS], &mut driver);
        self.used.clear();
        self.used.extend(done.shell.map(|r| (0, r.used)));
        self.used.extend(done.children.iter().map(|(h, r)| (*h, r.used)));
        self.system.stopped().is_none()
    }

    /// The layout of machine `serial`'s page as last laid out.
    pub fn layout(&self, serial: u32) -> Option<Arc<Layout>> {
        self.io.get(&serial).and_then(|io| io.shown.lock().unwrap().clone())
    }
}
