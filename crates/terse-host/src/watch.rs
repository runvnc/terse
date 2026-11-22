//! terse-host's watch over its frames (`PERFORMANCE.md` R14, R15;
//! `RECORD.md`). Its devices are compiled in, so the host times the parts
//! of its own frame loop, not separate processes: each machine's frame
//! (its emulation, its devices' calls within it and their host-side work
//! for it), the rest of the emulation stage (`core`), taking the machines'
//! screens and drawing (`compose`), acquiring and presenting the picture
//! (`present`), and how late the frame began after its tick or the last picture (`outside`).
//! The frame loop thread's CPU time and run-queue wait are read around
//! each part, and the whole process's while it draws (the renderer's
//! threads). `rule.rs` judges the frame; the host carries out what it asks.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use terse_perflog::env::Environment;
use terse_perflog::probe::{Pressure, ProcProbe, ProcSample};
use terse_perflog::record::{self, Line, MEM_EVERY, Readings, Recorder};
use terse_perflog::{Component, FrameSample, Part};

use crate::rule::{Culprit, Enforcer, Judged, Opened};

/// One machine's share of a frame.
#[derive(Clone, Debug)]
struct MachinePart {
    serial: u32,
    image: [u8; 32],
    wall: Duration,
    sample: Option<ProcSample>,
    opened: bool,
    /// The device whose work was the most of it: 1 (the emulation and
    /// the calls within it), 7 (its page's host-side work), 4 (its content
    /// requests handed over).
    slot: u8,
}

/// The emulation and upload of the step just run.
struct Stepped {
    emulate: Duration,
    upload: Duration,
    late: Duration,
    /// The frame loop's wait in the emulation, the upload, and before the
    /// frame began (since the last frame was presented).
    waits: Option<(u64, u64, u64)>,
}

/// The watch.
pub struct HostWatch {
    pub rule: Enforcer,
    rec: Option<Recorder>,
    /// The record's directory (a machine alone's snapshot goes there).
    pub dir: Option<PathBuf>,
    thread: ProcProbe,
    process: ProcProbe,
    psi: Pressure,
    thread_wait: bool,
    process_wait: bool,
    at_begin: Option<ProcSample>,
    psi_begin: Option<u64>,
    at_machine: Option<ProcSample>,
    at_emulated: Option<ProcSample>,
    at_draw: Option<ProcSample>,
    last_presented: Option<ProcSample>,
    machines: Vec<MachinePart>,
    /// The machine parts of the frame judged last.
    judged: Vec<MachinePart>,
    step: Option<Stepped>,
    opened: Opened,
    /// The next frame is the first at a new size (the first of all, too).
    resized: bool,
    /// The host's frame number: one per frame presented.
    pub frame: u64,
    started: Instant,
    start_written: bool,
    run_start: Instant,
    late: u64,
}

fn wait(a: Option<ProcSample>, b: Option<ProcSample>) -> Option<u64> {
    Some(a?.since(&b?).wait_ns)
}

impl HostWatch {
    /// `dir`: the record's directory (`None`: judge only).
    pub fn new(dir: Option<PathBuf>, default_dir: bool, renderer: &str, mode: &str, started: Instant) -> HostWatch {
        let rec = dir.as_ref().map(|d| Recorder::open(d, "terse-host", default_dir));
        let thread = ProcProbe::this_thread();
        let process = ProcProbe::process(std::process::id());
        let psi = Pressure::default();
        if let Some(r) = &rec {
            r.add(Line::Run { date: record::utc_date(), mode: mode.to_string() });
            r.add(Line::Env(Environment::detect(renderer)));
            // Every device is built in: each reports the host's identity.
            r.add(Line::host_devices(&[2, 3, 4, 6, 7, 9, 10]));
            // What the system tells: the frame loop thread's readings, and
            // the whole process's for the drawing (its threads).
            r.add(Line::Readings { component: Component::Core, readings: Readings { cpu: thread.has_cpu(), wait: thread.has_wait(), stall: psi.available() } });
            r.add(Line::Readings { component: Component::Compose, readings: Readings { cpu: process.has_cpu(), wait: process.has_wait(), stall: false } });
        }
        HostWatch {
            rule: Enforcer::default(),
            resized: true,
            rec,
            dir,
            thread_wait: thread.has_wait(),
            process_wait: process.has_wait(),
            thread,
            process,
            psi,
            at_begin: None,
            psi_begin: None,
            at_machine: None,
            at_emulated: None,
            at_draw: None,
            last_presented: None,
            machines: Vec::new(),
            judged: Vec::new(),
            step: None,
            opened: Opened::default(),
            frame: 0,
            started,
            start_written: false,
            run_start: Instant::now(),
            late: 0,
        }
    }

    pub fn add(&self, line: Line) {
        if let Some(r) = &self.rec {
            r.add(line);
        }
    }

    /// A step (a system frame) begins.
    pub fn begin(&mut self) {
        self.at_begin = self.thread.sample();
        self.psi_begin = self.psi.memory_full_ns();
        self.machines.clear();
        self.at_emulated = None;
    }

    /// Machine `serial`'s frame begins.
    pub fn machine_begin(&mut self) {
        self.at_machine = self.thread.sample();
    }

    /// Machine `serial` (running `image`) ran its frame: `parts`, the time
    /// of its emulation (with its devices' calls), its page's host-side
    /// work and its content requests'.
    pub fn machine_end(&mut self, serial: u32, image: [u8; 32], parts: [Duration; 3]) {
        let now = self.thread.sample();
        let sample = match (now, self.at_machine.take()) {
            (Some(a), Some(b)) => Some(a.since(&b)),
            _ => None,
        };
        let slot = [1, 7, 4][(0..3).max_by_key(|&i| (parts[i], std::cmp::Reverse(i))).unwrap_or(0)];
        let opened = self.opened.ran(serial);
        self.machines.push(MachinePart { serial, image, wall: parts.iter().sum(), sample, opened, slot });
    }

    /// The machines' frames have run.
    pub fn emulated(&mut self) {
        self.at_emulated = self.thread.sample();
    }

    /// The step ran: emulation and upload took this long, and it began
    /// `late` after its tick.
    pub fn stepped(&mut self, emulate: Duration, upload: Duration, late: Duration) {
        let now = self.thread.sample();
        let waits = match (wait(self.at_emulated, self.at_begin), wait(now, self.at_emulated)) {
            (Some(e), Some(u)) => Some((e, u, wait(self.at_begin, self.last_presented).unwrap_or(0))),
            _ => None,
        };
        self.step = Some(Stepped { emulate, upload, late, waits });
    }

    /// The picture is about to be drawn and presented.
    pub fn drawing(&mut self) {
        self.at_draw = self.process.sample();
    }

    /// The look was shed to `level` (`why`: `late`, `recover`, `check`).
    pub fn shed(&self, level: u8, why: &str) {
        self.add(Line::Shed { frame: self.frame, level: level as u32, why: why.into() });
    }

    /// The window's surface changed size, or the window moved to another
    /// monitor or its scale changed: the next frame drawn is the first at
    /// the new size, and its blame and the next two frames' are excused
    /// as `resize` (R15, `terse_perflog::Resizes`).
    pub fn resized(&mut self) {
        self.resized = true;
    }

    /// The device to name when machine `serial` is stopped for the frame
    /// judged last (`MachinePart::slot`).
    pub fn slot_of(&self, serial: u32) -> u8 {
        self.judged.iter().find(|m| m.serial == serial).map_or(1, |m| m.slot)
    }

    /// The parts of the frame just presented, and who each is.
    fn sample(&mut self, acquire: Duration, render: Duration) -> Option<(FrameSample, Vec<Culprit>, Vec<bool>)> {
        let s = self.step.take()?;
        let ns = |d: Duration| d.as_nanos() as u64;
        let now = self.thread.sample();
        let pnow = self.process.sample();
        let draw_process = self.at_draw.take();
        self.last_presented = now;
        let thread_ok = self.thread_wait && s.waits.is_some();
        let mut parts = Vec::with_capacity(self.machines.len() + 4);
        let mut who = Vec::with_capacity(parts.capacity());
        let mut opened = Vec::with_capacity(parts.capacity());
        let mut machines_wall = 0;
        let mut machines_wait = 0;
        for m in &self.machines {
            let wall = ns(m.wall);
            machines_wall += wall;
            let starved = m.sample.filter(|_| thread_ok).map(|x| x.wait_ns);
            machines_wait += starved.unwrap_or(0);
            parts.push(Part { component: Component::Core, wall_ns: wall, run_ns: m.sample.filter(|_| thread_ok).map(|x| x.run_ns), starved_ns: starved, image: Some(m.image) });
            who.push(Culprit::Machine(m.serial));
            opened.push(m.opened);
        }
        let (emulate_wait, upload_wait, before_wait) = s.waits.unwrap_or((0, 0, 0));
        let thread_starved = |w: u64| thread_ok.then_some(w);
        // The drawing's starvation: the whole process's (the renderer's
        // threads and the frame loop's) while it drew and presented.
        let draw_wait = if self.process_wait { wait(pnow, draw_process) } else { None };
        let compose_starved = match (thread_starved(upload_wait), draw_wait) {
            (Some(u), Some(d)) => Some(u + d),
            _ => None,
        };
        let mut host = |c: Component, wall: u64, starved: Option<u64>| {
            parts.push(Part { component: c, wall_ns: wall, run_ns: None, starved_ns: starved, image: None });
            who.push(Culprit::Host(c));
            opened.push(false);
        };
        host(Component::Core, ns(s.emulate).saturating_sub(machines_wall), thread_starved(emulate_wait.saturating_sub(machines_wait)));
        host(Component::Compose, ns(s.upload) + ns(render), compose_starved);
        host(Component::Present, ns(acquire), thread_starved(0));
        host(Component::Outside, ns(s.late), thread_starved(before_wait));
        let wall = parts.iter().map(|p| p.wall_ns).sum();
        let memory = match (self.psi.memory_full_ns(), self.psi_begin) {
            (Some(a), Some(b)) => Some(a.saturating_sub(b)),
            _ => None,
        };
        let size_changed = std::mem::take(&mut self.resized);
        let sample = FrameSample { frame: self.frame, wall_ns: wall, parts, host_starved_ns: thread_ok.then_some(0), memory_stall_ns: memory, size_changed };
        Some((sample, who, opened))
    }

    /// The frame's picture was presented: judges the frame and records it.
    /// `can_shed`: the look has a level left to shed. Returns what the rule
    /// asks for (`None`: no step ran before this picture).
    pub fn presented(&mut self, acquire: Duration, render: Duration, can_shed: bool) -> Option<Judged> {
        let (sample, who, opened) = self.sample(acquire, render)?;
        let judged = self.rule.judge(&sample, &who, &opened, can_shed);
        if let Some(l) = &judged.late {
            self.late += 1;
            self.add(match judged.excused {
                Some(why) => Line::excused(l, why.name()),
                None => Line::late(l),
            });
        }
        if !self.start_written {
            self.start_written = true;
            self.add(Line::Start { ms: (self.started.elapsed().as_secs_f64() * 10_000.0).round() / 10.0, cold: "auto".into() });
        }
        if self.frame % MEM_EVERY == 0
            && let Some(k) = self.process.rss_kib()
        {
            self.add(Line::Mem { seconds: self.run_start.elapsed().as_secs(), component: Component::Core, rss_kib: k });
        }
        self.judged = std::mem::take(&mut self.machines);
        self.frame += 1;
        Some(judged)
    }
}

impl Drop for HostWatch {
    fn drop(&mut self) {
        self.add(Line::End { frames: self.frame, late: self.late, seconds: self.run_start.elapsed().as_secs() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terse_perflog::Verdict;

    /// One step as the window runs it: machines 0 and 3, then the drawing.
    fn step(w: &mut HostWatch, machine3: Duration, render: Duration) -> Option<Judged> {
        let ms = Duration::from_millis;
        w.begin();
        for (serial, d) in [(0, ms(1)), (3, machine3)] {
            w.machine_begin();
            w.machine_end(serial, [serial as u8; 32], [d, Duration::ZERO, Duration::ZERO]);
        }
        w.emulated();
        w.stepped(ms(1) + machine3 + ms(1), ms(1), Duration::ZERO);
        w.drawing();
        w.presented(Duration::ZERO, render, false)
    }

    #[test]
    fn records_late_frames_by_part() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = HostWatch::new(Some(dir.path().to_path_buf()), false, "software", "window", Instant::now());
        let ms = Duration::from_millis;
        // Past the first frames: frames 30 to 33 draw for 30 ms,
        // frames 40 to 43 machine 3 takes 30 ms.
        for n in 0..50 {
            let render = if (30..34).contains(&n) { ms(30) } else { ms(2) };
            let machine = if (40..44).contains(&n) { ms(30) } else { ms(1) };
            step(&mut w, machine, render).unwrap();
        }
        w.shed(1, "late");
        // Dropping the watch writes the record to its end. (Its file is
        // named once the writer has hashed this test program, which a
        // slow computer may not have done within `path`'s second.)
        drop(w);
        let path = std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "log")).expect("the record");
        let lines = record::parse(&std::fs::read_to_string(path).unwrap());
        let late: Vec<(u64, Component, String)> =
            lines.iter().filter_map(|l| if let Line::Late { frame, component, image, .. } = l { Some((*frame, *component, image.clone())) } else { None }).collect();
        // The verdict depends on whether this computer was busy; the part
        // does not.
        for (frame, component, image) in &late {
            match frame {
                30..34 => assert_eq!(*component, Component::Compose),
                40..44 => assert_eq!((*component, image.as_str()), (Component::Core, "0303030303030303")),
                _ => panic!("{late:?}"),
            }
        }
        assert!(late.len() >= 4, "{late:?}");
        assert!(lines.iter().any(|l| matches!(l, Line::Readings { component: Component::Core, .. })));
        assert!(lines.iter().any(|l| matches!(l, Line::Shed { level: 1, .. })));
        assert!(lines.iter().any(|l| matches!(l, Line::End { frames: 50, .. })));
    }

    #[test]
    fn the_first_frames_are_excused() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = HostWatch::new(Some(dir.path().to_path_buf()), false, "software", "window", Instant::now());
        let ms = Duration::from_millis;
        // The first frame of all is the first at its size: it and the two
        // after it are excused as `resize` (unless the computer was busy).
        for _ in 0..3 {
            let j = step(&mut w, ms(1), ms(40)).unwrap();
            assert!(j.acts.is_empty() && j.late.unwrap().verdict != Verdict::Blamed, "{j:?}");
        }
        // A size change later: the same.
        for _ in 3..10 {
            step(&mut w, ms(1), ms(2)).unwrap();
        }
        w.resized();
        for _ in 10..13 {
            let j = step(&mut w, ms(1), ms(40)).unwrap();
            assert!(j.acts.is_empty() && j.late.unwrap().verdict != Verdict::Blamed, "{j:?}");
        }
        drop(w);
        let path = std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "log")).expect("the record");
        let lines = record::parse(&std::fs::read_to_string(path).unwrap());
        assert!(lines.iter().any(|l| matches!(l, Line::Late { verdict: Verdict::Resize | Verdict::Starved, .. })), "{lines:?}");
    }

    /// The cost of watching a frame of a shell and 8 machines: the
    /// readings around each machine and part, and the judging (printed;
    /// bounded loosely, for an unoptimised build on a busy computer).
    #[test]
    fn the_cost_of_watching() {
        let mut w = HostWatch::new(None, false, "software", "window", Instant::now());
        let n = 600;
        let t = Instant::now();
        for _ in 0..n {
            w.begin();
            for serial in 0..9 {
                w.machine_begin();
                w.machine_end(serial, [serial as u8; 32], [Duration::from_micros(100), Duration::ZERO, Duration::ZERO]);
            }
            w.emulated();
            w.stepped(Duration::from_millis(1), Duration::from_millis(1), Duration::ZERO);
            w.drawing();
            w.presented(Duration::ZERO, Duration::from_millis(2), true).unwrap();
        }
        let per = t.elapsed() / n;
        println!("watching a frame of 9 machines: {per:?}");
        assert!(per < Duration::from_millis(2), "{per:?}");
    }
}
