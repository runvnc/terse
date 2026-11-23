//! The core's watch over its drivers (`PERFORMANCE.md` R14, R15;
//! `RECORD.md`). The core hands each driver its work and waits for the
//! reply, so it can time every driver on its own: per frame, the time it
//! waited for each driver's frame work ([`crate::bus::Acct`]), and from
//! the operating system the driver process's CPU time and the time its
//! threads were runnable but not running (starved). The judge
//! (`terse_perflog::Judge`) says which driver, if any, made a late frame
//! late; the rule says when the host warns and when it stops the
//! machines that use it. Everything goes to the record, off the frame
//! loop.
//!
//! The host's own parts have their time on a CPU too (`RECORD.md` 4), so
//! that a memory stall excuses no more of it than it can explain: `core`
//! is the frame loop thread's CPU time from the frame's start to the
//! machines' frames run, `compose` the whole process's from then until
//! presenting begins (composing runs on a pool of threads), `present` the
//! frame loop thread's while presenting. The process's CPU time includes
//! threads that do not compose (sound, the content service): their time is
//! counted as composing's, which can only excuse less. `outside` is the
//! frame loop's waits for drivers' other work (services, `Acct::other_ns`):
//! blocked on another process by design, its time on a CPU is not read.

use std::path::PathBuf;
use std::time::Instant;

use terse_perflog::env::Environment;
use terse_perflog::probe::{Pressure, ProcProbe, ProcSample};
use terse_perflog::record::{self, BLOCK_NS, Line, MEM_EVERY, Readings, Recorder};
use terse_perflog::{Action, Component, FrameSample, Identity, Judge, Part, Rule};

use crate::bus::{Acct, Bus};
use crate::stats::Histogram;

/// How to watch.
#[derive(Clone, Debug)]
pub struct WatchConfig {
    /// Where the record goes (`None`: no record).
    pub record: Option<PathBuf>,
    /// Whether the rule acts (warns, stops); otherwise it only records.
    pub act: bool,
    /// `window`, `realtime` or `headless`.
    pub mode: &'static str,
    /// `drivers` (the drivers draw) or `none`.
    pub renderer: &'static str,
    /// When the process started (for the start-up line).
    pub started: Instant,
    /// The record is the user's (the default directory): the start-up
    /// line can say whether this is the first run since the computer
    /// started (a marker in the runtime directory).
    pub default_dir: bool,
    pub rule: Rule,
}

/// One driver as the monitor sees it.
struct Watched {
    component: Component,
    probe: Option<ProcProbe>,
    before: Option<ProcSample>,
    /// What the system reads of it (once, at the start).
    readings: Readings,
}

/// The watch.
pub struct Monitor {
    pub judge: Judge,
    pub act: bool,
    pub rec: Option<Recorder>,
    pub dir: Option<PathBuf>,
    drivers: Vec<Watched>,
    core: ProcProbe,
    core_wait: bool,
    core_before: Option<ProcSample>,
    core_cpu: bool,
    me_cpu: bool,
    /// The frame loop thread and the process when the machines' frames
    /// had run, and when presenting began.
    core_emulated: Option<ProcSample>,
    me_emulated: Option<ProcSample>,
    core_presenting: Option<ProcSample>,
    me_presenting: Option<ProcSample>,
    me: ProcProbe,
    psi: Pressure,
    psi_before: Option<u64>,
    /// The host's frame number: one per frame watched.
    pub frame: u64,
    t0: Instant,
    /// When emulation ended, and the drivers' waits by then.
    emulated: Option<(Instant, u64)>,
    started: Instant,
    start_written: bool,
    run_start: Instant,
    pub late: u64,
    /// The monitor's own cost per frame.
    pub overhead: Histogram,
    /// The window's surface size last composed, and whether this frame is
    /// the first composed at it (R15: the frames after a resize).
    surface: Option<(u32, u32)>,
    size_changed: bool,
    /// This frame's wait for the window system's buffer.
    display_wait_ns: u64,
    /// Late frames blamed on the host's own components (by component).
    pub own_late: std::collections::BTreeMap<Component, u64>,
}

impl Monitor {
    /// Starts watching `bus`'s drivers: the record's head (run, host,
    /// computer, devices).
    pub fn new(cfg: &WatchConfig, bus: &Bus) -> Monitor {
        let rec = cfg.record.as_ref().map(|d| Recorder::open(d, "terse-bus", cfg.default_dir));
        if let Some(r) = &rec {
            r.add(Line::Run { date: record::utc_date(), mode: cfg.mode.to_string() });
            r.add(Line::Env(Environment::detect(cfg.renderer)));
            for d in &bus.procs {
                r.add(Line::Device {
                    component: Component::Device(d.component),
                    slots: d.slots.clone(),
                    identity: Identity { name: d.info.name.clone(), version: d.info.version.clone(), build: d.info.build.clone() },
                });
            }
        }
        let drivers: Vec<Watched> = bus.procs.iter().map(|d| {
                let probe = d.pid.map(ProcProbe::process);
                let p = probe.as_ref();
                let readings = Readings { cpu: p.is_some_and(ProcProbe::has_cpu), wait: p.is_some_and(ProcProbe::has_wait), stall: false, idle: p.is_some_and(ProcProbe::infers_wait) };
                Watched { component: Component::Device(d.component), probe, before: None, readings }
            }).collect();
        let core = ProcProbe::this_thread();
        let me = ProcProbe::process(std::process::id());
        let psi = Pressure::default();
        if let Some(r) = &rec {
            // What the operating system tells us (RECORD.md 4): without a
            // driver's CPU wait, it is blamed by its waits alone and never
            // stopped.
            r.add(Line::Readings { component: Component::Core, readings: Readings { cpu: core.has_cpu(), wait: core.has_wait(), stall: psi.available(), idle: core.infers_wait() } });
            // Composing: the whole process's time on a CPU (its pool of
            // threads); its starvation is not read apart from the frame
            // loop's.
            r.add(Line::Readings { component: Component::Compose, readings: Readings { cpu: me.has_cpu(), wait: false, stall: false, idle: false } });
            for w in &drivers {
                r.add(Line::Readings { component: w.component, readings: w.readings });
            }
        }
        Monitor {
            judge: Judge::new(cfg.rule),
            act: cfg.act,
            rec,
            dir: cfg.record.clone(),
            drivers,
            core_wait: core.has_wait(),
            core_cpu: core.has_cpu(),
            core,
            core_before: None,
            core_emulated: None,
            me_emulated: None,
            core_presenting: None,
            me_presenting: None,
            me_cpu: me.has_cpu(),
            me,
            psi,
            psi_before: None,
            frame: 0,
            t0: Instant::now(),
            emulated: None,
            started: cfg.started,
            start_written: false,
            run_start: Instant::now(),
            late: 0,
            overhead: Histogram::default(),
            surface: None,
            size_changed: false,
            display_wait_ns: 0,
            own_late: Default::default(),
        }
    }

    pub fn add(&self, line: Line) {
        if let Some(r) = &self.rec {
            r.add(line);
        }
    }

    /// A frame begins: the accounts start again and the counters are read.
    pub fn begin(&mut self, bus: &mut Bus) {
        let t = Instant::now();
        for d in &mut bus.procs {
            d.acct = Acct::default();
        }
        for w in &mut self.drivers {
            w.before = w.probe.as_mut().and_then(ProcProbe::sample);
        }
        self.core_before = self.core.sample();
        self.psi_before = self.psi.memory_full_ns();
        self.emulated = None;
        self.core_emulated = None;
        self.me_emulated = None;
        self.core_presenting = None;
        self.me_presenting = None;
        self.overhead.add(t.elapsed());
        self.t0 = Instant::now();
    }

    /// The frame is composed for a window's surface of `size` (called each
    /// frame in a window, before [`Monitor::end`]): a new size is recorded
    /// and the rule excuses the frames just after it (R15).
    pub fn surface(&mut self, size: (u32, u32)) {
        if self.surface != Some(size) {
            self.surface = Some(size);
            self.size_changed = true;
            self.add(Line::Resize { frame: self.frame, width: size.0, height: size.1 });
        }
    }

    /// The frame's present waited this long for the window system to give
    /// a buffer back: left out of the frame's time (R14).
    pub fn display_wait(&mut self, ns: u64) {
        self.display_wait_ns += ns;
    }

    /// The machines' frames have run (composing follows).
    pub fn emulated(&mut self, bus: &Bus) {
        let waits = bus.procs.iter().map(|d| d.acct.frame_ns + d.acct.other_ns).sum();
        self.core_emulated = self.core.sample();
        self.me_emulated = self.me.sample();
        self.emulated = Some((Instant::now(), waits));
    }

    /// The composed picture is being handed to the display (called in a
    /// window, after composing; headless there is none).
    pub fn presenting(&mut self) {
        self.core_presenting = self.core.sample();
        self.me_presenting = self.me.sample();
    }

    /// The frame's picture is presented (`present_ns`: handing it to the
    /// display, 0 headless). Judges the frame; returns what the rule asks
    /// for (an empty list unless the rule acts). `image` names the
    /// program of an instance.
    pub fn end(&mut self, bus: &Bus, present_ns: u64, image: impl Fn(u32) -> Option<[u8; 32]>) -> Vec<Action> {
        let end = Instant::now();
        // The wait for the window system to give a buffer back is its
        // pacing (like a vertical blank), not the frame's work (R14).
        let wall = (end.duration_since(self.t0).as_nanos() as u64).saturating_sub(std::mem::take(&mut self.display_wait_ns));
        let t = Instant::now();
        let (emul_end, waits_then) = self.emulated.unwrap_or((end, 0));
        let emul = emul_end.duration_since(self.t0).as_nanos() as u64;
        let waits_all: u64 = bus.procs.iter().map(|d| d.acct.frame_ns + d.acct.other_ns).sum();
        let mut parts = Vec::with_capacity(bus.procs.len() + 4);
        let mut counted = 0;
        let mut blocks = Vec::new();
        for (d, w) in bus.procs.iter().zip(&mut self.drivers) {
            let now = w.probe.as_mut().and_then(ProcProbe::sample);
            let delta = match (now, w.before) {
                (Some(a), Some(b)) => Some(a.since(&b)),
                _ => None,
            };
            counted += d.acct.frame_ns;
            parts.push(Part {
                component: w.component,
                wall_ns: d.acct.frame_ns,
                run_ns: delta.filter(|_| w.readings.cpu).map(|s| s.run_ns),
                starved_ns: delta.filter(|_| w.readings.wait).map(|s| s.wait_within(d.acct.frame_ns)),
                image: d.acct.top_instance().and_then(&image),
            });
            if d.acct.longest.0 >= BLOCK_NS {
                blocks.push(Line::Block { frame: self.frame, ms: d.acct.longest.0 as f64 / 1e6, component: w.component, what: d.acct.longest.1.to_string() });
            }
        }
        for b in blocks {
            self.add(b);
        }
        let core_now = self.core.sample();
        let core = match (core_now, self.core_before) {
            (Some(a), Some(b)) => Some(a.since(&b)),
            _ => None,
        };
        let core_ns = emul.saturating_sub(waits_then);
        let compose_ns = wall.saturating_sub(emul).saturating_sub(present_ns).saturating_sub(waits_all - waits_then);
        // Their time on a CPU. Without a present (headless), presenting
        // begins and ends at the frame's end.
        let me_presenting = if self.core_presenting.is_some() { self.me_presenting } else { self.me.sample() };
        let core_presenting = self.core_presenting.or(core_now);
        let cpu = |a: Option<ProcSample>, b: Option<ProcSample>| Some(a?.since(&b?).cpu_ns);
        let (thread_cpu, process_cpu) = (self.core_cpu, self.me_cpu);
        let core_run = cpu(self.core_emulated, self.core_before).filter(|_| thread_cpu);
        let compose_run = cpu(me_presenting, self.me_emulated).filter(|_| process_cpu);
        let present_run = cpu(core_now, core_presenting).filter(|_| thread_cpu);
        parts.push(Part { component: Component::Core, wall_ns: core_ns, run_ns: core_run, starved_ns: None, image: image(0) });
        parts.push(Part { run_ns: compose_run, ..Part::new(Component::Compose, compose_ns) });
        parts.push(Part { run_ns: present_run, ..Part::new(Component::Present, present_ns) });
        let own_total = core_ns + compose_ns + present_ns + counted;
        parts.push(Part::new(Component::Outside, wall.saturating_sub(own_total)));
        let memory = match (self.psi.memory_full_ns(), self.psi_before) {
            (Some(a), Some(b)) => Some(a.saturating_sub(b)),
            _ => None,
        };
        let size_changed = std::mem::take(&mut self.size_changed);
        // The frame loop's own starvation: within its own time, not the
        // time it waited for the drivers (blocked by design, and already
        // the drivers' parts; an inferred wait would count it twice).
        let host_starved = core.filter(|_| self.core_wait).map(|c| c.wait_within(wall.saturating_sub(waits_all)));
        let sample = FrameSample { frame: self.frame, wall_ns: wall, parts, host_starved_ns: host_starved, memory_stall_ns: memory, size_changed };
        let (late, actions) = self.judge.judge(&sample);
        if let Some(l) = &late {
            self.late += 1;
            self.add(Line::late(l));
            if l.verdict == terse_perflog::Verdict::Blamed && !l.top.component.is_device() {
                *self.own_late.entry(l.top.component).or_default() += 1;
            }
        }
        if !self.start_written {
            self.start_written = true;
            let ms = self.started.elapsed().as_secs_f64() * 1000.0;
            self.add(Line::Start { ms: (ms * 10.0).round() / 10.0, cold: "auto".into() });
        }
        if self.frame % MEM_EVERY == 0 && self.rec.is_some() {
            let s = self.run_start.elapsed().as_secs();
            if let Some(k) = self.me.rss_kib() {
                self.add(Line::Mem { seconds: s, component: Component::Core, rss_kib: k });
            }
            for w in &self.drivers {
                if let Some(k) = w.probe.as_ref().and_then(ProcProbe::rss_kib) {
                    self.add(Line::Mem { seconds: s, component: w.component, rss_kib: k });
                }
            }
        }
        self.frame += 1;
        self.overhead.add(t.elapsed());
        if self.act { actions } else { Vec::new() }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        let seconds = self.run_start.elapsed().as_secs();
        self.add(Line::End { frames: self.frame, late: self.late, seconds });
    }
}
