//! terse-host's watch over its own stages (`PERFORMANCE.md` R14; `RECORD.md`).
//! Its devices are compiled in, so the host times its own stages, not
//! separate processes: the emulation (the machines and the devices' calls:
//! the `core`), taking the machines' screens and drawing (`compose`),
//! acquiring and presenting the picture (`present`), and how late the
//! frame began after its tick (`outside`: the event loop held by something
//! else). The frame loop thread's run-queue wait says when the computer
//! was busy. This is less independent than the bus's watch: a stage that
//! slows another (a lock, the caches) is not told apart, and every
//! component is the host's own: the host records and shows, and never
//! stops a machine for it (R15).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use terse_perflog::env::Environment;
use terse_perflog::probe::{Pressure, ProcProbe, ProcSample};
use terse_perflog::record::{self, Line, MEM_EVERY, Recorder};
use terse_perflog::{Action, Component, FrameSample, Judge, Part};

/// The watch.
pub struct HostWatch {
    judge: Judge,
    rec: Option<Recorder>,
    thread: ProcProbe,
    me: ProcProbe,
    psi: Pressure,
    before: Option<(ProcSample, Option<u64>)>,
    /// The last step's stages: emulation, upload, lateness of its start.
    step: Option<(Duration, Duration, Duration)>,
    frame: u64,
    started: Instant,
    start_written: bool,
    run_start: Instant,
    late: u64,
}

impl HostWatch {
    /// `dir`: the record's directory (`None`: judge and show only).
    pub fn new(dir: Option<PathBuf>, default_dir: bool, renderer: &str, mode: &str, started: Instant) -> HostWatch {
        let rec = dir.as_ref().map(|d| Recorder::open(d, "terse-host", default_dir));
        if let Some(r) = &rec {
            r.add(Line::Run { date: record::utc_date(), mode: mode.to_string() });
            r.add(Line::Env(Environment::detect(renderer)));
            // Every device is built in: each reports the host's identity.
            r.add(Line::host_devices(&[2, 3, 4, 6, 7, 9, 10]));
        }
        HostWatch {
            judge: Judge::default(),
            rec,
            thread: ProcProbe::this_thread(),
            me: ProcProbe::process(std::process::id()),
            psi: Pressure::default(),
            before: None,
            step: None,
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
        self.before = self.thread.sample().map(|s| (s, self.psi.memory_full_ns()));
    }

    /// The step ran: emulation and upload took this long, and it began
    /// `late` after its tick.
    pub fn stepped(&mut self, emulate: Duration, upload: Duration, late: Duration) {
        self.step = Some((emulate, upload, late));
    }

    /// The look was shed to `level` (`why`: `late`, `recover`).
    pub fn shed(&self, level: u8, why: &str) {
        self.add(Line::Shed { frame: self.frame, level: level as u32, why: why.into() });
    }

    /// The frame's picture was presented: judges the frame. Returns lines
    /// to show (the host's own parts: recorded and shown, never stopped).
    pub fn presented(&mut self, acquire: Duration, render: Duration) -> Vec<String> {
        let Some((emulate, upload, late)) = self.step.take() else { return Vec::new() };
        let ns = |d: Duration| d.as_nanos() as u64;
        let (starved, memory) = match (self.thread.sample(), self.before.take()) {
            (Some(now), Some((b, psi))) => (Some(now.since(&b).wait_ns).filter(|_| self.thread.has_wait()), self.psi.memory_full_ns().zip(psi).map(|(a, b)| a.saturating_sub(b))),
            _ => (None, None),
        };
        let parts = vec![
            Part::new(Component::Core, ns(emulate)),
            Part::new(Component::Compose, ns(upload) + ns(render)),
            Part::new(Component::Present, ns(acquire)),
            Part::new(Component::Outside, ns(late)),
        ];
        let wall = parts.iter().map(|p| p.wall_ns).sum();
        let sample = FrameSample { frame: self.frame, wall_ns: wall, parts, host_starved_ns: starved, memory_stall_ns: memory, size_changed: false };
        let (l, actions) = self.judge.judge(&sample);
        if let Some(l) = &l {
            self.late += 1;
            self.add(Line::late(l));
        }
        if !self.start_written {
            self.start_written = true;
            self.add(Line::Start { ms: (self.started.elapsed().as_secs_f64() * 10_000.0).round() / 10.0, cold: "auto".into() });
        }
        if self.frame % MEM_EVERY == 0
            && let Some(k) = self.me.rss_kib()
        {
            self.add(Line::Mem { seconds: self.run_start.elapsed().as_secs(), component: Component::Core, rss_kib: k });
        }
        let mut out = Vec::new();
        for a in actions {
            let (c, count) = match a {
                Action::Warn { component, count } | Action::Stop { component, count } => (component, count),
            };
            self.add(Line::Warn { frame: self.frame, component: c, count });
            out.push(format!("terse-host: the host's own {c} made {count} frames late (a host cannot stop itself; `terse perf log` shows the record)"));
        }
        self.frame += 1;
        out
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

    #[test]
    fn records_late_frames_by_stage() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = HostWatch::new(Some(dir.path().to_path_buf()), false, "software", "window", Instant::now());
        let ms = Duration::from_millis;
        for n in 0..10 {
            w.begin();
            // Frames 5 to 8 draw for 30 ms.
            w.stepped(ms(1), ms(1), Duration::ZERO);
            let render = if (5..9).contains(&n) { ms(30) } else { ms(2) };
            let shown = w.presented(ms(0), render);
            assert_eq!(shown.len(), usize::from(n == 7), "{n}: {shown:?}");
        }
        w.shed(1, "late");
        let path = w.rec.as_ref().unwrap().path().unwrap();
        drop(w);
        let lines = record::parse(&std::fs::read_to_string(path).unwrap());
        let late: Vec<(u64, Component)> = lines.iter().filter_map(|l| if let Line::Late { frame, component, .. } = l { Some((*frame, *component)) } else { None }).collect();
        // The verdict depends on whether this computer was busy; the stage
        // does not.
        assert_eq!(late.iter().map(|l| l.1).collect::<Vec<_>>(), vec![Component::Compose; late.len()]);
        assert!(late.iter().all(|l| (5..9).contains(&l.0)), "{late:?}");
        assert!(lines.iter().any(|l| matches!(l, Line::Shed { level: 1, .. })));
        assert!(lines.iter().any(|l| matches!(l, Line::End { frames: 10, .. })));
    }
}
