//! `terse-host --drive <script|->`: the window driven by a script, as a
//! user would drive it, and watched (STATE.md 6.5, README "Driving the
//! window").
//!
//! The image runs in the ordinary window: the real renderer, the page
//! thread laying out and the figures rasterising beside the frames, 60
//! frames a second of real time. Input enters where the window system's
//! does (keys with the text a keyboard gives, the pointer in window pixels
//! through the window's mapping, the wheel, the window's size), and what is
//! presented is read back (the picture handed to the window system). The
//! script is the events file of STATE.md 6.2 with its extension (lines
//! without a frame number, `wait`, `type`) and the commands of 6.5:
//! pictures, dumps, watching frames for changes, expectations.
//!
//! It never shows a window on the desktop it is started from unless told
//! to ([`display`]): on Linux it starts a headless Wayland compositor of
//! its own (weston) and runs itself in it ([`nest`]); on Windows and macOS,
//! which have no headless display of their own, it refuses. Only
//! `TERSE_DRIVE_DISPLAY=current` uses the current display (a CI machine's,
//! whose desktop nobody uses).
//!
//! The script comes from a file, run to its end, or from a FIFO or `-`
//! (standard input) line by line as written, for an agent driving it by
//! hand; each line a FIFO's writer closes it with is read, and the FIFO is
//! opened again for the next. Reports go to standard output.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use terse_core::script::{self, char_key, key_code, split_line, words};
use terse_core::system::{CONTROLLERS, Controller, Event, buttons, modifiers};

/// What the drive asks the window to do before a frame, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// The modifier keys held (SPEC §events bits), as the window system
    /// reports them before a key.
    Modifiers(u8),
    /// A key going down or up (HID usage), with the text it types.
    Key { usage: u8, pressed: bool, repeat: bool, text: Option<String> },
    /// Text events without a key (an input method's).
    Text(String),
    /// The pointer moves to window pixels (`px`) or to the centre of a
    /// screen unit (mapped to pixels through the window's picture).
    Pointer { x: f64, y: f64, px: bool },
    /// The mouse buttons held (SPEC §events bits): each that changes goes
    /// down or up.
    Buttons(u8),
    /// Wheel steps, positive down and right (SPEC §events).
    Wheel { v: i32, h: i32 },
    /// An event for the system as it is (feeds-updated).
    Event(Event),
    /// The window's inner size, in pixels.
    Resize(u32, u32),
    /// End the run.
    Quit,
}

/// What the drive reads from the host after a frame.
pub trait Probe {
    /// `terminal`, `page`, `vectors` or `state` of machine `handle` (`None`:
    /// the focused machine, or the machine alone).
    fn dump(&self, what: &str, handle: Option<u8>) -> Result<String, String>;
    /// The window's picture size in pixels.
    fn viewport(&self) -> (u32, u32);
}

/// One frame's work of the machines: units of the frame budget each used.
pub type Budget = [(u32, u64)];

/// A picture span being watched.
struct Watch {
    /// Presented frames still to look at.
    left: u32,
    total: u32,
    /// `settle`: ends after this many unchanged pictures in a row.
    settle: Option<u32>,
    still: u32,
    /// `expect idle`: pixels that may differ.
    tolerance: Option<u64>,
    save: Option<PathBuf>,
    every: u32,
    seen: u32,
    changed: Vec<Change>,
    blinks: Vec<Change>,
    started_frame: u32,
    line: String,
}

/// A picture that differs from the one before it: where and how much.
#[derive(Clone, Debug)]
struct Change {
    frame: u32,
    pixels: u64,
    bbox: (u32, u32, u32, u32),
    max: u8,
}

/// The tiles pictures are compared by, for blinks.
const TILE: u32 = 32;

pub struct Drive {
    lines: Receiver<Line>,
    /// Lines read and not yet carried out.
    queue: VecDeque<(usize, String)>,
    /// The script has no more lines.
    ended: bool,
    /// From a file: the run ends with its last line.
    file: bool,
    /// The next line waits for this frame, or this time.
    resume_at: u32,
    resume_time: Option<Instant>,
    /// Characters still to type, one a frame, and keys held (usage,
    /// frames left, frames to the next repeat).
    typing: VecDeque<char>,
    held: Option<(u8, u32, u32, u8)>,
    /// Observations to make of the next picture presented (frame asked for).
    want_picture: Vec<(u32, String)>,
    /// The thread that looks at the pictures (so that comparing and
    /// writing them does not make the window's frames late), and how many
    /// of the observations sent to it are not yet done.
    worker: std::sync::mpsc::Sender<Job>,
    results: Receiver<Msg>,
    outstanding: usize,
    /// A `watch` goes on beside the lines that follow it until this frame.
    watching: u32,
    /// Late frames noted.
    late: u32,
    /// The picture thread has done all it was given (at the end).
    flushed: bool,
    pads: [Controller; CONTROLLERS],
    pads_set: bool,
    failures: Vec<String>,
    /// Budget use per machine serial: (worst, last, frame of the worst).
    budget: Vec<(u32, u64, u64, u32)>,
    budget_since: u32,
    /// The host's notes on slow frames (late frames, the rule's acts).
    slow: Vec<String>,
    frame: u32,
    out: std::io::Stdout,
}

/// A picture: `soft::base::pack`ed pixels, rows packed.
struct Picture {
    size: (u32, u32),
    px: Vec<u32>,
}

impl Picture {
    fn rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.px.len() * 4);
        for &p in &self.px {
            let [r, g, b] = crate::soft::base::unpack(p);
            out.extend_from_slice(&[r, g, b, 255]);
        }
        out
    }
}

/// A presented picture as the renderer gives it.
pub enum Raw {
    /// The software renderer's pixels (`soft::base::pack`).
    Packed(Vec<u32>),
    /// Rows of 4 bytes a pixel, `stride` bytes apart, BGRA if `bgra`.
    Rows { data: Vec<u8>, stride: usize, bgra: bool },
}

impl Raw {
    fn packed(self, (w, h): (u32, u32)) -> Vec<u32> {
        match self {
            Raw::Packed(px) => px,
            Raw::Rows { data, stride, bgra } => {
                let mut out = Vec::with_capacity((w * h) as usize);
                for row in data.chunks(stride).take(h as usize) {
                    for p in row[..w as usize * 4].chunks_exact(4) {
                        out.push(if bgra { crate::soft::base::pack(p[2], p[1], p[0]) } else { crate::soft::base::pack(p[0], p[1], p[2]) });
                    }
                }
                out
            }
        }
    }
}

/// What the picture thread is given.
enum Job {
    /// An observation of the pictures (`shot`, `watch`, `settle`, `expect idle`).
    Observe(u32, String),
    Picture(u32, (u32, u32), Raw),
    /// The run ends: report a watch still going, then answer `Flushed`.
    Flush,
}

/// What it answers.
enum Msg {
    Say(String),
    Fail(String),
    /// An observation is done.
    Done,
    Flushed,
}

/// The picture thread's state.
struct Looker {
    queue: VecDeque<(u32, String)>,
    watch: Option<Watch>,
    /// The last pictures, newest first (for blinks: up to 4).
    history: VecDeque<Picture>,
    out: std::sync::mpsc::Sender<Msg>,
}

enum Line {
    Text(usize, String),
    End,
}

/// Opens the script: a file read whole, or a FIFO or standard input read
/// as lines arrive.
fn open(path: &str) -> Result<(Receiver<Line>, bool), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    if path == "-" {
        std::thread::spawn(move || {
            for (i, l) in std::io::stdin().lock().lines().enumerate() {
                let Ok(l) = l else { break };
                if tx.send(Line::Text(i + 1, l)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Line::End);
        });
        return Ok((rx, false));
    }
    #[cfg(unix)]
    let fifo = {
        use std::os::unix::fs::FileTypeExt;
        std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?.file_type().is_fifo()
    };
    #[cfg(not(unix))]
    let fifo = false;
    if !fifo {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        for (i, l) in text.lines().enumerate() {
            let _ = tx.send(Line::Text(i + 1, l.to_string()));
        }
        let _ = tx.send(Line::End);
        return Ok((rx, true));
    }
    let path = path.to_string();
    std::thread::spawn(move || {
        let mut n = 0;
        // Each writer's lines; then the FIFO is opened again for the next.
        loop {
            let Ok(f) = std::fs::File::open(&path) else { break };
            for l in std::io::BufReader::new(f).lines() {
                let Ok(l) = l else { break };
                n += 1;
                if tx.send(Line::Text(n, l)).is_err() {
                    return;
                }
            }
        }
        let _ = tx.send(Line::End);
    });
    Ok((rx, false))
}

/// Checks a script's lines before the window opens (a file only).
pub fn check(path: &str) -> Result<(), String> {
    if path == "-" {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?.file_type().is_fifo() {
            return Ok(());
        }
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    for (i, l) in text.lines().enumerate() {
        parse(l).map_err(|e| format!("{path}: line {}: {e}", i + 1))?;
    }
    Ok(())
}

/// A line's frame (if written), command and arguments; `None` if blank.
fn parse(line: &str) -> Result<Option<(Option<u32>, String, String)>, String> {
    let Some((frame, command, args)) = split_line(line)? else { return Ok(None) };
    let w = words(args);
    let key = |i: usize| -> Result<u8, String> {
        let name = w.get(i).ok_or("missing key")?;
        key_code(name).ok_or(format!("unknown key {name}"))
    };
    let num = |i: usize, what: &str| -> Result<i64, String> {
        let v = w.get(i).ok_or(format!("missing {what}"))?;
        v.parse::<i64>().map_err(|_| format!("bad {what} {v}"))
    };
    match command {
        "down" | "up" | "press" => {
            key(0)?;
            mods(&w[1..])?;
        }
        "hold" => {
            key(0)?;
            num(1, "frames")?;
            mods(w.get(2..).unwrap_or(&[]))?;
        }
        "pointer" | "click" | "pointer-px" | "click-px" => {
            num(0, "x")?;
            num(1, "y")?;
        }
        "drag" | "drag-px" => {
            for (i, what) in ["x0", "y0", "x1", "y1"].iter().enumerate() {
                num(i, what)?;
            }
        }
        "wheel" => {
            num(0, "vertical steps")?;
        }
        "wait" => {
            num(0, "frame count")?;
        }
        "sleep" => {
            num(0, "milliseconds")?;
        }
        "resize" => {
            num(0, "width")?;
            num(1, "height")?;
        }
        "shot" => {
            w.first().ok_or("missing file")?;
        }
        "dump" => match w.first().copied() {
            Some("terminal" | "page" | "vectors" | "state") => {}
            _ => return Err("dump terminal|page|vectors|state [<handle>] [> <file>]".into()),
        },
        "watch" | "settle" => {}
        "expect" => match w.first().copied() {
            Some("idle") => {
                num(1, "frame count")?;
            }
            Some("terminal" | "page" | "vectors" | "state") if matches!(w.get(1).copied(), Some("contains" | "lacks")) => {}
            Some("budget") => {
                num(1, "percent")?;
            }
            Some("no-slow") => {}
            _ => {
                return Err(
                    "expect idle <frames> [tolerance <pixels>] | expect <terminal|page|vectors|state> contains|lacks <text> | expect budget <percent> | expect no-slow"
                        .into(),
                );
            }
        },
        "text" | "type" | "click-at" | "feeds-updated" | "home" | "pad" | "perf" | "budget" | "quit" | "say" => {}
        c => return Err(format!("unknown command {c:?}")),
    }
    Ok(Some((frame, command.to_string(), args.to_string())))
}

fn mods(w: &[&str]) -> Result<u8, String> {
    w.iter().try_fold(0, |bits, m| {
        Ok(bits
            | match *m {
                "shift" => modifiers::SHIFT,
                "ctrl" | "control" => modifiers::CONTROL,
                "alt" => modifiers::ALT,
                "meta" => modifiers::META,
                _ => return Err(format!("unknown modifier {m}")),
            })
    })
}

/// The modifier keys (HID usages) a set of modifier bits is held with.
const MODIFIER_KEYS: [(u8, u8); 4] = [(modifiers::CONTROL, 0xE0), (modifiers::SHIFT, 0xE1), (modifiers::ALT, 0xE2), (modifiers::META, 0xE3)];

/// The text a key types on a US keyboard with these modifiers (none with
/// Control, Alt or Meta, nor for keys that type no character).
fn key_text(usage: u8, mods: u8) -> Option<String> {
    if mods & (modifiers::CONTROL | modifiers::ALT | modifiers::META) != 0 {
        return None;
    }
    let shift = mods & modifiers::SHIFT != 0;
    let c = (32u8..127).map(char::from).find(|&c| char_key(c) == Some((usage, shift)))?;
    Some(c.to_string())
}

impl Drive {
    pub fn new(path: &str) -> Result<Drive, String> {
        let (lines, file) = open(path)?;
        let (worker, jobs) = std::sync::mpsc::channel::<Job>();
        let (tx, results) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("drive pictures".into())
            .spawn(move || {
                let mut l = Looker { queue: VecDeque::new(), watch: None, history: VecDeque::new(), out: tx };
                while let Ok(j) = jobs.recv() {
                    match j {
                        Job::Observe(f, what) => l.queue.push_back((f, what)),
                        Job::Picture(f, size, raw) => l.picture(f, size, raw),
                        Job::Flush => {
                            if let Some(w) = l.watch.take() {
                                let f = w.started_frame + w.seen;
                                l.say(format!("(the run ended {} pictures before the watch did)", w.left));
                                l.report(w, f);
                            }
                            let _ = l.out.send(Msg::Flushed);
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Drive {
            lines,
            queue: VecDeque::new(),
            ended: false,
            file,
            resume_at: 0,
            resume_time: None,
            typing: VecDeque::new(),
            held: None,
            want_picture: Vec::new(),
            worker,
            results,
            outstanding: 0,
            watching: 0,
            late: 0,
            flushed: false,
            pads: [Controller::default(); CONTROLLERS],
            pads_set: false,
            failures: Vec::new(),
            budget: Vec::new(),
            budget_since: 0,
            slow: Vec::new(),
            frame: 0,
            out: std::io::stdout(),
        })
    }

    fn say(&mut self, s: &str) {
        let mut o = self.out.lock();
        let _ = writeln!(o, "{s}");
        let _ = o.flush();
    }

    fn fail(&mut self, s: String) {
        self.say(&format!("frame {}: FAIL: {s}", self.frame));
        self.failures.push(s);
    }

    /// The controllers, once a `pad` line has set one.
    pub fn controllers(&self) -> Option<[Controller; CONTROLLERS]> {
        self.pads_set.then_some(self.pads)
    }

    /// Whether the next picture presented is wanted.
    pub fn wants_picture(&self) -> bool {
        !self.want_picture.is_empty() || self.outstanding > 0 || self.frame <= self.watching
    }

    /// The host noted a slow frame or acted on one (PERFORMANCE.md R15):
    /// a late frame is counted, an act said.
    pub fn slow_frame(&mut self, note: String) {
        if note.starts_with("the look shed") {
            // Presentation only: said, not counted as slow.
            return self.say(&format!("frame {}: {note}", self.frame));
        }
        if note.starts_with("late frame") {
            self.late += 1;
            // The window system's own delay (the compositor, a busy
            // computer) is not the program's or this host's: counted only.
            if note.contains(": Outside ") {
                return;
            }
        } else {
            self.say(&format!("frame {}: slow: {note}", self.frame));
        }
        self.slow.push(note);
    }

    /// What the picture thread said.
    fn results(&mut self) {
        while let Ok(m) = self.results.try_recv() {
            match m {
                Msg::Say(s) => self.say(&s),
                Msg::Fail(s) => {
                    self.say(&format!("FAIL: {s}"));
                    self.failures.push(s);
                }
                Msg::Done => self.outstanding = self.outstanding.saturating_sub(1),
                Msg::Flushed => self.flushed = true,
            }
        }
    }

    /// The exit status: 1 if an expectation failed.
    pub fn status(&self) -> Option<ExitCode> {
        (!self.failures.is_empty()).then(|| ExitCode::from(1))
    }

    /// Prints the summary at the end.
    pub fn finish(&mut self) {
        // What the picture thread has still to say (it may be behind).
        let _ = self.worker.send(Job::Flush);
        let t = Instant::now();
        while !self.flushed && t.elapsed() < Duration::from_secs(300) {
            self.results();
            std::thread::sleep(Duration::from_millis(5));
        }
        self.results();
        if self.late > 0 {
            self.say(&format!("drive: {} late frames (the record has them)", self.late));
        }
        let n = self.failures.len();
        if n == 0 {
            self.say("drive: done, no expectation failed");
        } else {
            self.say(&format!("drive: done, {n} expectation{} failed:", if n == 1 { "" } else { "s" }));
            for f in self.failures.clone() {
                self.say(&format!("  {f}"));
            }
        }
    }

    /// Before frame `frame` runs: the lines whose time has come, as actions.
    pub fn before_frame(&mut self, frame: u32) -> Vec<Action> {
        self.frame = frame;
        let mut out = Vec::new();
        if let Some(c) = self.typing.pop_front() {
            match char_key(c) {
                Some((k, shift)) => {
                    let m = if shift { modifiers::SHIFT } else { 0 };
                    self.key(&mut out, k, m, true, if c.is_control() { None } else { Some(c.to_string()) });
                    self.key(&mut out, k, m, false, None);
                }
                None => out.push(Action::Text(c.to_string())),
            }
            return out;
        }
        if let Some((k, left, next, m)) = self.held {
            // A held key repeats as a keyboard's does: after 30 frames,
            // then every 2 (25 a second).
            if left == 0 {
                self.key(&mut out, k, m, false, None);
                self.held = None;
            } else {
                if next == 0 {
                    out.push(Action::Key { usage: k, pressed: true, repeat: true, text: key_text(k, m) });
                }
                self.held = Some((k, left - 1, if next == 0 { 1 } else { next - 1 }, m));
                return out;
            }
        }
        self.results();
        if self.outstanding > 0 || !self.want_picture.is_empty() || frame < self.resume_at {
            return out;
        }
        if let Some(t) = self.resume_time {
            if Instant::now() < t {
                return out;
            }
            self.resume_time = None;
        }
        loop {
            if self.queue.is_empty() {
                match self.lines.try_recv() {
                    Ok(Line::Text(n, l)) => self.queue.push_back((n, l)),
                    Ok(Line::End) | Err(TryRecvError::Disconnected) => {
                        self.ended = true;
                    }
                    Err(TryRecvError::Empty) => {}
                }
            }
            let Some((n, line)) = self.queue.pop_front() else {
                if self.ended && self.held.is_none() {
                    out.push(Action::Quit);
                }
                return out;
            };
            let parsed = match parse(&line) {
                Ok(Some(p)) => p,
                Ok(None) => continue,
                Err(e) => {
                    self.fail(format!("line {n}: {e}: {line}"));
                    if self.file {
                        out.push(Action::Quit);
                        return out;
                    }
                    continue;
                }
            };
            let (at, command, args) = parsed;
            if let Some(f) = at
                && f > frame
            {
                // A numbered line waits for its frame.
                self.queue.push_front((n, line));
                self.resume_at = f;
                return out;
            }
            if self.run(frame, n, &command, &args, &mut out) {
                return out;
            }
        }
    }

    /// Presses or releases `usage` with `mods` as a keyboard would: the
    /// modifier keys first, then the key, then the modifiers released.
    fn key(&mut self, out: &mut Vec<Action>, usage: u8, mods: u8, pressed: bool, text: Option<String>) {
        if pressed {
            let mut held = 0;
            for (bit, k) in MODIFIER_KEYS {
                if mods & bit != 0 && usage != k {
                    out.push(Action::Key { usage: k, pressed: true, repeat: false, text: None });
                    held |= bit;
                    out.push(Action::Modifiers(held));
                }
            }
            out.push(Action::Key { usage, pressed: true, repeat: false, text });
        } else {
            out.push(Action::Key { usage, pressed: false, repeat: false, text: None });
            let mut held = mods;
            for (bit, k) in MODIFIER_KEYS.iter().rev() {
                if mods & bit != 0 && usage != *k {
                    held &= !bit;
                    out.push(Action::Key { usage: *k, pressed: false, repeat: false, text: None });
                    out.push(Action::Modifiers(held));
                }
            }
        }
    }

    /// Carries out one line; true if the next must wait (for a frame, a
    /// picture, or time).
    fn run(&mut self, frame: u32, n: usize, command: &str, args: &str, out: &mut Vec<Action>) -> bool {
        let w = words(args);
        let int = |i: usize| w.get(i).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
        match command {
            "down" | "up" | "press" => {
                let k = key_code(w[0]).unwrap_or(0);
                let m = mods(&w[1..]).unwrap_or(0);
                if command != "up" {
                    let text = key_text(k, m);
                    self.key(out, k, m, true, text);
                }
                if command != "down" {
                    self.key(out, k, m, false, None);
                }
            }
            "hold" => {
                let k = key_code(w[0]).unwrap_or(0);
                let m = mods(w.get(2..).unwrap_or(&[])).unwrap_or(0);
                self.key(out, k, m, true, key_text(k, m));
                self.held = Some((k, int(1).max(1) as u32, 30, m));
                return true;
            }
            "text" => out.push(Action::Text(args.to_string())),
            "type" => {
                self.typing.extend(args.chars());
                return true;
            }
            "pointer" | "pointer-px" => {
                let px = command == "pointer-px";
                out.push(Action::Pointer { x: int(0) as f64, y: int(1) as f64, px });
                out.push(Action::Buttons(int(2) as u8 & 7));
            }
            "click" | "click-px" => {
                out.push(Action::Pointer { x: int(0) as f64, y: int(1) as f64, px: command == "click-px" });
                out.push(Action::Buttons(buttons::PRIMARY));
                out.push(Action::Buttons(0));
            }
            "drag" | "drag-px" => {
                // Pressed at the start, moved there in 8 frames, released.
                let px = command == "drag-px";
                let (x0, y0, x1, y1) = (int(0) as f64, int(1) as f64, int(2) as f64, int(3) as f64);
                out.push(Action::Pointer { x: x0, y: y0, px });
                out.push(Action::Buttons(buttons::PRIMARY));
                for i in 1..=8 {
                    let t = i as f64 / 8.0;
                    let line = format!("{} pointer{} {} {} 1", frame + i, if px { "-px" } else { "" }, (x0 + (x1 - x0) * t).round(), (y0 + (y1 - y0) * t).round());
                    self.queue.insert(i as usize - 1, (n, line));
                }
                self.queue.insert(8, (n, format!("{} pointer{} {} {} 0", frame + 9, if px { "-px" } else { "" }, x1, y1)));
                return true;
            }
            "wheel" => out.push(Action::Wheel { v: int(0) as i32, h: int(1) as i32 }),
            "feeds-updated" => out.push(Action::Event(Event::feeds_updated())),
            "home" => {
                out.push(Action::Key { usage: 0x45, pressed: true, repeat: false, text: None });
                out.push(Action::Key { usage: 0x45, pressed: false, repeat: false, text: None });
            }
            "pad" => {
                if let Ok(Some((_, c))) = pad_line(args) {
                    self.pads[c.0] = c.1;
                    self.pads_set = true;
                }
            }
            "wait" => {
                self.resume_at = frame + int(0).max(0) as u32;
                return int(0) > 0;
            }
            "sleep" => {
                self.resume_time = Some(Instant::now() + Duration::from_millis(int(0).max(0) as u64));
                return true;
            }
            "resize" => {
                out.push(Action::Resize(int(0).max(1) as u32, int(1).max(1) as u32));
                // The window system answers in a frame or two.
                self.resume_at = frame + 3;
                return true;
            }
            "quit" => {
                out.push(Action::Quit);
                return true;
            }
            "say" => {
                let s = format!("frame {frame}: {args}");
                self.say(&s);
            }
            "watch" | "settle" | "expect" | "shot" | "dump" | "perf" | "budget" => {
                self.want_picture.push((frame, format!("{command} {args}")));
                // Observation lines in a row look at the same picture.
                if self.queue.is_empty() {
                    match self.lines.try_recv() {
                        Ok(Line::Text(n, l)) => self.queue.push_back((n, l)),
                        Ok(Line::End) | Err(TryRecvError::Disconnected) => self.ended = true,
                        Err(TryRecvError::Empty) => {}
                    }
                }
                let next = self.queue.front().and_then(|(_, l)| parse(l).ok().flatten());
                return !next.is_some_and(|(f, c, _)| f.is_none() && matches!(c.as_str(), "shot" | "dump" | "expect" | "perf" | "budget"));
            }
            _ => {}
        }
        false
    }

    /// After the frame ran: what is read from the machines (dumps,
    /// expectations on them), and the frame's budget use.
    pub fn after_frame(&mut self, probe: &dyn Probe, used: &Budget) {
        for &(serial, u) in used {
            match self.budget.iter_mut().find(|b| b.0 == serial) {
                Some(b) => {
                    b.2 = u;
                    if u > b.1 {
                        b.1 = u;
                        b.3 = self.frame;
                    }
                }
                None => self.budget.push((serial, u, u, self.frame)),
            }
        }
        let _ = probe;
    }

    /// A picture was presented for frame `frame` (`raw`: read back if
    /// [`Self::wants_picture`] said so).
    pub fn presented(&mut self, frame: u32, raw: Option<Raw>, probe: &dyn Probe) {
        self.frame = frame;
        self.results();
        // Observations asked for at or before this frame, in order; one of
        // the pictures holds the rest until it is done.
        while self.outstanding == 0 && self.want_picture.first().is_some_and(|w| w.0 <= frame) {
            let (_, what) = self.want_picture.remove(0);
            let (command, args) = what.split_once(' ').unwrap_or((what.as_str(), ""));
            let picture = matches!(command, "shot" | "watch" | "settle") || (command == "expect" && words(args).first() == Some(&"idle"));
            if picture {
                let _ = self.worker.send(Job::Observe(frame, what.clone()));
                if command == "watch" {
                    // It watches while the next lines run (typing, say).
                    let n = words(args).first().and_then(|v| v.parse().ok()).unwrap_or(60u32);
                    self.watching = frame + n + 120;
                } else {
                    self.outstanding += 1;
                }
            } else {
                self.observe(&what, frame, probe);
            }
        }
        if let Some(raw) = raw {
            let _ = self.worker.send(Job::Picture(frame, probe.viewport(), raw));
        }
    }

    /// An observation that does not need the picture.
    fn observe(&mut self, what: &str, frame: u32, probe: &dyn Probe) {
        let (command, args) = what.split_once(' ').unwrap_or((what, ""));
        let w = words(args);
        match command {
            "dump" => {
                let to_file = args.split_once('>').map(|(_, f)| f.trim().to_string());
                let handle = w.get(1).filter(|h| **h != ">").and_then(|h| h.parse::<u8>().ok());
                let text = match probe.dump(w[0], handle) {
                    Ok(t) => t,
                    Err(e) => format!("({e})\n"),
                };
                match to_file {
                    Some(f) => {
                        let line = match std::fs::write(&f, &text) {
                            Ok(()) => format!("frame {frame}: dump {} written to {f}", w[0]),
                            Err(e) => format!("frame {frame}: dump: {f}: {e}"),
                        };
                        self.say(&line);
                    }
                    None => self.say(&format!("-- frame {frame}: dump {} --\n{}-- end of dump --", w[0], text)),
                }
            }
            "expect" => self.expect(&w, args, frame, probe),
            "perf" => {
                let s = format!("frame {frame}: {} late frames, {} other slow-frame notes", self.late, self.slow.len() as u32 - self.late);
                self.say(&s);
                for n in self.slow.clone().into_iter().filter(|n| !n.starts_with("late frame")) {
                    self.say(&format!("  {n}"));
                }
            }
            "budget" => {
                let s = self.budget_lines(frame);
                self.say(&s);
                for b in &mut self.budget {
                    b.1 = 0;
                }
                self.budget_since = frame;
            }
            _ => {}
        }
    }

    fn budget_lines(&self, frame: u32) -> String {
        let total = terse_core::BUDGET_PER_FRAME as f64;
        let mut s = format!("frame {frame}: frame budget since frame {} (125,000 units a frame):", self.budget_since);
        for (serial, worst, last, at) in &self.budget {
            s += &format!("\n  machine serial {serial}: worst {:.1}% (frame {at}), last {:.1}%", *worst as f64 / total * 100.0, *last as f64 / total * 100.0);
        }
        s
    }

    fn expect(&mut self, w: &[&str], args: &str, frame: u32, probe: &dyn Probe) {
        match w.first().copied() {
            Some(what @ ("terminal" | "page" | "vectors" | "state")) => {
                let want = w[1] == "contains";
                let needle = args.splitn(3, char::is_whitespace).nth(2).unwrap_or("").trim();
                match probe.dump(what, None) {
                    Ok(text) => {
                        if text.contains(needle) != want {
                            self.fail(format!("expect {what} {} {needle:?}: the {what} is:\n{text}", w[1]));
                        } else {
                            self.say(&format!("frame {frame}: ok: {what} {} {needle:?}", w[1]));
                        }
                    }
                    Err(e) => self.fail(format!("expect {what}: {e}")),
                }
            }
            Some("budget") => {
                let pct: f64 = w.get(1).and_then(|v| v.parse().ok()).unwrap_or(100.0);
                let total = terse_core::BUDGET_PER_FRAME as f64;
                let over: Vec<String> = self
                    .budget
                    .iter()
                    .filter(|b| b.1 as f64 / total * 100.0 > pct)
                    .map(|b| format!("machine serial {} used {:.1}% in frame {}", b.0, b.1 as f64 / total * 100.0, b.3))
                    .collect();
                if over.is_empty() {
                    self.say(&format!("frame {frame}: ok: budget at most {pct}%"));
                } else {
                    self.fail(format!("expect budget {pct}%: {}", over.join("; ")));
                }
            }
            Some("no-slow") => {
                if self.slow.is_empty() {
                    self.say(&format!("frame {frame}: ok: no slow frames"));
                } else {
                    let s = self.slow.join("; ");
                    self.fail(format!("expect no-slow: {s}"));
                }
            }
            _ => {}
        }
    }
}

impl Looker {
    fn say(&self, s: String) {
        let _ = self.out.send(Msg::Say(s));
    }

    fn picture(&mut self, frame: u32, size: (u32, u32), raw: Raw) {
        let px = raw.packed(size);
        if px.len() != (size.0 * size.1) as usize {
            return;
        }
        let picture = Picture { size, px };
        if self.history.front().is_some_and(|p| p.size != size) {
            self.history.clear();
        }
        if let Some(mut w) = self.watch.take() {
            w.seen += 1;
            let (change, blink) = match self.history.front() {
                Some(prev) => compare(&picture, prev, &self.history, frame),
                None => (None, None),
            };
            if let Some(c) = &change {
                w.changed.push(c.clone());
                w.still = 0;
                if let Some(dir) = &w.save
                    && w.changed.len() <= 20
                {
                    let _ = std::fs::create_dir_all(dir);
                    if let Err(e) = crate::write_png(&dir.join(format!("frame-{frame:06}.png")), &picture.rgba(), size) {
                        self.say(format!("frame {frame}: {e}"));
                    }
                }
            } else {
                w.still += 1;
            }
            if let Some(b) = &blink {
                w.blinks.push(b.clone());
            }
            if let Some(dir) = &w.save
                && w.every > 0
                && w.seen % w.every == 0
            {
                let _ = std::fs::create_dir_all(dir);
                let _ = crate::write_png(&dir.join(format!("every-{frame:06}.png")), &picture.rgba(), size);
            }
            w.left = w.left.saturating_sub(1);
            if w.left == 0 || w.settle.is_some_and(|k| w.still >= k) {
                let blocking = !w.line.starts_with("watch");
                self.report(w, frame);
                if blocking {
                    let _ = self.out.send(Msg::Done);
                }
            } else {
                self.watch = Some(w);
            }
        }
        self.history.push_front(picture);
        self.history.truncate(4);
        while (self.watch.is_none() || self.watch.as_ref().is_some_and(|w| w.line.starts_with("watch") && self.queue.front().is_some_and(|q| q.1.starts_with("shot"))))
            && self.queue.front().is_some_and(|q| q.0 <= frame)
        {
            let (_, what) = self.queue.pop_front().unwrap();
            self.observe(&what, frame);
        }
    }

    fn observe(&mut self, what: &str, frame: u32) {
        let (command, args) = what.split_once(' ').unwrap_or((what, ""));
        let w = words(args);
        let named = |name: &str| w.iter().position(|x| *x == name).and_then(|i| w.get(i + 1)).map(|s| s.to_string());
        let watch = |total: u32, line: String| Watch {
            left: total,
            total,
            settle: None,
            still: 0,
            tolerance: None,
            save: named("save").map(PathBuf::from),
            every: named("every").and_then(|v| v.parse().ok()).unwrap_or(0),
            seen: 0,
            changed: Vec::new(),
            blinks: Vec::new(),
            started_frame: frame,
            line,
        };
        match command {
            "shot" => {
                if let Some(p) = self.history.front() {
                    let path = PathBuf::from(w.first().copied().unwrap_or("shot.png"));
                    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let line = match crate::write_png(&path, &p.rgba(), p.size) {
                        Ok(()) => format!("frame {frame}: shot {} ({}x{})", path.display(), p.size.0, p.size.1),
                        Err(e) => format!("frame {frame}: shot: {e}"),
                    };
                    self.say(line);
                }
                let _ = self.out.send(Msg::Done);
                return;
            }
            "watch" => {
                let n = w.first().and_then(|v| v.parse().ok()).unwrap_or(60u32).max(1);
                self.watch = Some(watch(n, what.to_string()));
            }
            "settle" => {
                let mut x = watch(named("max").and_then(|v| v.parse().ok()).unwrap_or(600), what.to_string());
                x.settle = Some(named("still").and_then(|v| v.parse().ok()).unwrap_or(10));
                self.watch = Some(x);
            }
            "expect" => {
                let n = w.get(1).and_then(|v| v.parse().ok()).unwrap_or(60u32).max(1);
                let mut x = watch(n, what.to_string());
                x.tolerance = Some(named("tolerance").and_then(|v| v.parse().ok()).unwrap_or(0));
                self.watch = Some(x);
            }
            _ => {
                let _ = self.out.send(Msg::Done);
                return;
            }
        }
        // A watch compares pictures from this one on.
        self.history.truncate(1);
    }

    /// The report of a watch that ended at `frame`.
    fn report(&mut self, w: Watch, frame: u32) {
        let mut s = format!(
            "frame {}-{frame}: {}: {} pictures, {} changed",
            w.started_frame,
            w.line.trim(),
            w.seen,
            w.changed.len()
        );
        if let Some(k) = w.settle {
            s += &if w.still >= k { format!(", settled ({k} unchanged)") } else { format!(", not settled in {} frames", w.total) };
        }
        let union = |cs: &[Change]| {
            cs.iter().fold(None, |acc: Option<(u32, u32, u32, u32)>, c| {
                Some(match acc {
                    None => c.bbox,
                    Some(a) => (a.0.min(c.bbox.0), a.1.min(c.bbox.1), a.2.max(c.bbox.2), a.3.max(c.bbox.3)),
                })
            })
        };
        if let Some(b) = union(&w.changed) {
            let max = w.changed.iter().map(|c| c.max).max().unwrap_or(0);
            let px = w.changed.iter().map(|c| c.pixels).max().unwrap_or(0);
            s += &format!("; changes within x {}..{}, y {}..{}, at most {px} pixels and {max} levels a picture", b.0, b.2, b.1, b.3);
        }
        if !w.blinks.is_empty() {
            let b = union(&w.blinks).unwrap();
            s += &format!("; {} blinks (a part changed and changed back within 2 pictures) within x {}..{}, y {}..{}", w.blinks.len(), b.0, b.2, b.1, b.3);
        }
        self.say(s);
        for b in w.blinks.iter().take(50) {
            self.say(format!("  frame {}: blink in x {}..{}, y {}..{}", b.frame, b.bbox.0, b.bbox.2, b.bbox.1, b.bbox.3));
        }
        for c in w.changed.iter().take(12) {
            self.say(format!("  frame {}: {} pixels changed in x {}..{}, y {}..{}, by up to {}", c.frame, c.pixels, c.bbox.0, c.bbox.2, c.bbox.1, c.bbox.3, c.max));
        }
        if let Some(t) = w.tolerance {
            let bad: Vec<&Change> = w.changed.iter().filter(|c| c.pixels > t).collect();
            if !bad.is_empty() || !w.blinks.is_empty() {
                let f = bad.first().map_or(w.blinks.first().map_or(0, |b| b.frame), |c| c.frame);
                let _ = self.out.send(Msg::Fail(format!(
                    "{}: the picture changed while idle: {} of {} pictures (more than {t} pixels), {} blinks; first at frame {f}",
                    w.line.trim(),
                    bad.len(),
                    w.seen,
                    w.blinks.len()
                )));
            } else {
                self.say(format!("frame {frame}: ok: {}", w.line.trim()));
            }
        }
    }
}

/// `pad` arguments as STATE.md 6.2 has them.
fn pad_line(args: &str) -> Result<Option<(u32, (usize, Controller))>, String> {
    let s = script::Script::parse(&format!("0 pad {args}"))?;
    let c = s.controllers(0);
    let n: usize = words(args).first().and_then(|v| v.parse().ok()).ok_or("bad controller")?;
    Ok(Some((0, (n, c[n]))))
}

/// How `now` differs from `prev`, the picture before it: pixels, their
/// box, the largest difference of a channel; and the box of its TILE ×
/// TILE tiles that changed from `prev` back to what they were 2 or 3
/// pictures before (`history`, newest first: a blink). Rows are compared
/// whole first, so a still picture costs little.
fn compare(now: &Picture, prev: &Picture, history: &VecDeque<Picture>, frame: u32) -> (Option<Change>, Option<Change>) {
    if now.px == prev.px {
        return (None, None);
    }
    let (w, h) = now.size;
    let (tw, th) = (w.div_ceil(TILE), h.div_ceil(TILE));
    let row = |_p: &Picture, y: u32, x0: u32, x1: u32| -> std::ops::Range<usize> { (y * w + x0) as usize..(y * w + x1) as usize };
    let (mut n, mut max) = (0u64, 0u8);
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    let mut blink: Option<(u32, u32, u32, u32, u64)> = None;
    for ty in 0..th {
        let (ya, yb) = (ty * TILE, ((ty + 1) * TILE).min(h));
        if now.px[row(now, ya, 0, w).start..row(now, yb - 1, 0, w).end] == prev.px[row(prev, ya, 0, w).start..row(prev, yb - 1, 0, w).end] {
            continue;
        }
        for tx in 0..tw {
            let (xa, xb) = (tx * TILE, ((tx + 1) * TILE).min(w));
            let same = |p: &Picture| (ya..yb).all(|y| now.px[row(now, y, xa, xb)] == p.px[row(p, y, xa, xb)]);
            if same(prev) {
                continue;
            }
            for y in ya..yb {
                let r = row(now, y, xa, xb);
                let (a, b) = (&prev.px[r.clone()], &now.px[r]);
                if a == b {
                    continue;
                }
                for (i, (pa, pb)) in a.iter().zip(b).enumerate() {
                    if pa != pb {
                        let (ca, cb) = (crate::soft::base::unpack(*pa), crate::soft::base::unpack(*pb));
                        let d = (0..3).map(|c| ca[c].abs_diff(cb[c])).max().unwrap();
                        let x = xa + i as u32;
                        n += 1;
                        max = max.max(d);
                        x0 = x0.min(x);
                        y0 = y0.min(y);
                        x1 = x1.max(x + 1);
                        y1 = y1.max(y + 1);
                    }
                }
            }
            if history.iter().skip(1).take(2).any(|p| p.size == now.size && same(p)) {
                let b = blink.get_or_insert((u32::MAX, u32::MAX, 0, 0, 0));
                *b = (b.0.min(xa), b.1.min(ya), b.2.max(xb), b.3.max(yb), b.4 + ((xb - xa) * (yb - ya)) as u64);
            }
        }
    }
    let change = (n > 0).then_some(Change { frame, pixels: n, bbox: (x0, y0, x1, y1), max });
    let blink = blink.map(|b| Change { frame, pixels: b.4, bbox: (b.0, b.1, b.2, b.3), max: 0 });
    (change, blink)
}

/// Where a drive's window goes ([`display`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Display {
    /// In a headless compositor of its own (Linux: weston).
    Nest,
    /// On the display it was started from: asked for
    /// (`TERSE_DRIVE_DISPLAY=current`), or already inside its own.
    Current,
    /// Nowhere: the system has no headless display of its own (Windows,
    /// macOS), and a window on this desktop was not asked for.
    Refuse,
}

/// The rule: a drive never shows a window on the desktop it was started
/// from unless `TERSE_DRIVE_DISPLAY=current` says so (meant for a CI
/// machine, whose desktop nobody uses). On Linux it nests in weston; on a
/// system with no headless display of its own it refuses.
pub fn display(linux: bool, nested: bool, current: bool) -> Display {
    if nested || current {
        Display::Current
    } else if linux {
        Display::Nest
    } else {
        Display::Refuse
    }
}

/// Runs `terse-host` again with `args` inside a headless Wayland compositor
/// of its own (weston), so no window is shown on this desktop; returns its
/// exit status. `None`: the window goes on the current display
/// (`TERSE_DRIVE_DISPLAY=current`, or already inside). An error where it
/// would have to show a window on this desktop without being asked to.
pub fn nest(args: &[String], size: Option<(u32, u32)>) -> Option<Result<ExitCode, String>> {
    let nested_already = std::env::var_os("TERSE_DRIVE_NESTED").is_some();
    let current = std::env::var("TERSE_DRIVE_DISPLAY").is_ok_and(|v| v == "current");
    match display(cfg!(target_os = "linux"), nested_already, current) {
        Display::Current => None,
        Display::Nest => Some(nested(args, size)),
        Display::Refuse => Some(Err(format!(
            "--drive: {} has no headless display of its own, and a window on this desktop was not asked for; on a machine whose desktop nobody uses (CI), set TERSE_DRIVE_DISPLAY=current",
            std::env::consts::OS
        ))),
    }
}

fn nested(args: &[String], size: Option<(u32, u32)>) -> Result<ExitCode, String> {
    use std::process::{Command, Stdio};
    let runtime = std::env::temp_dir().join(format!("terse-drive-{}", std::process::id()));
    std::fs::create_dir_all(&runtime).map_err(|e| format!("{}: {e}", runtime.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700));
    }
    let (w, h) = size.unwrap_or((880, 660));
    // Room for the window and for resizing it.
    let (ow, oh) = (w.max(2560) + 64, h.max(1600) + 64);
    let socket = "terse-drive";
    let weston = Command::new("weston")
        .args(["--backend=headless", "--idle-time=0", "--no-config"])
        .arg(format!("--width={ow}"))
        .arg(format!("--height={oh}"))
        .arg(format!("--socket={socket}"))
        .env("XDG_RUNTIME_DIR", &runtime)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut weston = match weston {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&runtime);
            return Err(format!(
                "--drive: no headless compositor: weston: {e} (install weston, or set TERSE_DRIVE_DISPLAY=current to use this display: a window will show)"
            ));
        }
    };
    let t = Instant::now();
    while !runtime.join(socket).exists() {
        if t.elapsed() > Duration::from_secs(10) || weston.try_wait().ok().flatten().is_some() {
            let _ = weston.kill();
            let _ = weston.wait();
            let _ = std::fs::remove_dir_all(&runtime);
            return Err("--drive: the headless compositor (weston) did not start".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let status = Command::new(exe)
        .args(args)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("WAYLAND_DISPLAY", socket)
        .env("TERSE_DRIVE_NESTED", "1")
        .env("TERSE_AUDIO", "off")
        .env_remove("DISPLAY")
        .status();
    let _ = weston.kill();
    let _ = weston.wait();
    let _ = std::fs::remove_dir_all(&runtime);
    let status = status.map_err(|e| e.to_string())?;
    if let Some(c) = status.code() {
        return Ok(ExitCode::from(c as u8));
    }
    // Ended by a signal (a fault: SIGTERM, as terse run): the same.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if status.signal() == Some(15) {
            return Ok(crate::fault_exit());
        }
    }
    Ok(ExitCode::from(1))
}

/// Scratch directories for a drive that was given none: never the user's.
pub fn scratch() -> Result<PathBuf, String> {
    let d = std::env::temp_dir().join(format!("terse-drive-dirs-{}", std::process::id()));
    for s in ["store", "config", "data"] {
        std::fs::create_dir_all(d.join(s)).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    Ok(d)
}

/// Removes the scratch directories.
pub fn remove_scratch(d: &Path) {
    let _ = std::fs::remove_dir_all(d);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_on_this_desktop_only_when_asked_for() {
        // Linux: a compositor of its own, unless asked, or already in it.
        assert_eq!(display(true, false, false), Display::Nest);
        assert_eq!(display(true, false, true), Display::Current);
        assert_eq!(display(true, true, false), Display::Current);
        // Windows, macOS: no headless display, so nothing unless asked.
        assert_eq!(display(false, false, false), Display::Refuse);
        assert_eq!(display(false, false, true), Display::Current);
    }

    #[test]
    fn lines_check() {
        for ok in ["press a ctrl", "10 click 3 4", "type =SUM(A1:A3)", "shot a.png", "dump page > p.txt", "expect idle 60 tolerance 0", "expect page contains Total", "settle max 100", "drag 100 50 300 50", "hold down 40", "resize 1900 1000", "wait 3", "# x", ""] {
            assert!(parse(ok).is_ok(), "{ok}");
        }
        for bad in ["press nokey", "dump nothing", "expect what", "click 1", "jump"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn keys_type_what_a_keyboard_types() {
        assert_eq!(key_text(0x04, 0).as_deref(), Some("a"));
        assert_eq!(key_text(0x04, modifiers::SHIFT).as_deref(), Some("A"));
        assert_eq!(key_text(0x04, modifiers::CONTROL), None);
        assert_eq!(key_text(0x28, 0), None);
        assert_eq!(key_text(0x2E, modifiers::SHIFT).as_deref(), Some("+"));
        // An empty script, in a file of its own (there is no /dev/null on
        // Windows).
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("empty");
        std::fs::write(&script, "").unwrap();
        let mut d = Drive::new(script.to_str().unwrap()).unwrap();
        let mut out = Vec::new();
        d.key(&mut out, 0x0A, modifiers::CONTROL, true, None);
        d.key(&mut out, 0x0A, modifiers::CONTROL, false, None);
        assert_eq!(
            out,
            [
                Action::Key { usage: 0xE0, pressed: true, repeat: false, text: None },
                Action::Modifiers(modifiers::CONTROL),
                Action::Key { usage: 0x0A, pressed: true, repeat: false, text: None },
                Action::Key { usage: 0x0A, pressed: false, repeat: false, text: None },
                Action::Key { usage: 0xE0, pressed: false, repeat: false, text: None },
                Action::Modifiers(0),
            ]
        );
    }

    #[test]
    fn changes_and_blinks_are_found() {
        let size = (64, 32);
        let pic = |px: Vec<u32>| Picture { size, px };
        let a = vec![0u32; 64 * 32];
        let mut b = a.clone();
        b[5 * 64 + 40] = crate::soft::base::pack(200, 0, 0);
        let h: VecDeque<Picture> = [pic(a.clone())].into_iter().collect();
        let (c, k) = compare(&pic(b.clone()), &pic(a.clone()), &h, 7);
        let c = c.unwrap();
        assert_eq!((c.pixels, c.bbox, c.max), (1, (40, 5, 41, 6), 200));
        assert!(k.is_none());
        assert!(compare(&pic(a.clone()), &pic(a.clone()), &h, 7).0.is_none());
        // a, b, a: the tile at x 32..64 blinked.
        let h: VecDeque<Picture> = [pic(b.clone()), pic(a.clone())].into_iter().collect();
        let (_, k) = compare(&pic(a.clone()), &pic(b.clone()), &h, 9);
        assert_eq!(k.unwrap().bbox, (32, 0, 64, 32));
    }
}
