//! `terse-host --selftest`: the image runs in the normal window with vsync and
//! scripted input, and the host measures itself per frame. Nothing here
//! passes or fails; it prints numbers (see README, "Responsiveness").

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use terse_core::Script;

/// Budgets the report counts frames against: one 60 Hz frame, and the share
/// of it a host should need on a reasonable machine.
const FRAME_MS: f64 = 1000.0 / 60.0;
const TIGHT_MS: f64 = 4.0;

pub struct SelfTest {
    pub script: Script,
    pub frames: u32,
    pub csv: Option<PathBuf>,
    /// One entry per machine frame run.
    pub log: Vec<FrameLog>,
    last_present: Option<Instant>,
}

#[derive(Clone, Copy, Default)]
pub struct FrameLog {
    /// `Machine::run_frame` and handing content requests to the service.
    pub emulate: Duration,
    /// Uploading the terminal and the display list to the GPU.
    pub upload: Duration,
    /// When this machine frame was the one on screen: encoding, submitting
    /// and presenting it (the wait for a swapchain image is `acquire`).
    pub present: Option<Presented>,
}

#[derive(Clone, Copy)]
pub struct Presented {
    pub acquire: Duration,
    pub render: Duration,
    /// Since the previous present.
    pub interval: Option<Duration>,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// p50, p99 and max of `values` (in ms), or `None` if empty.
fn spread(mut values: Vec<f64>) -> Option<(f64, f64, f64)> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
    Some((at(0.5), at(0.99), values[values.len() - 1]))
}

impl SelfTest {
    pub fn new(script: Script, frames: u32, csv: Option<PathBuf>) -> Self {
        SelfTest { script, frames, csv, log: Vec::new(), last_present: None }
    }

    pub fn done(&self) -> bool {
        self.log.len() >= self.frames as usize
    }

    /// Records a present of the newest machine frame (only its first present).
    pub fn presented(&mut self, acquire: Duration, render: Duration, at: Instant) {
        let interval = self.last_present.map(|t| at - t);
        self.last_present = Some(at);
        if let Some(entry) = self.log.last_mut()
            && entry.present.is_none()
        {
            entry.present = Some(Presented { acquire, render, interval });
        }
    }

    /// Prints the summary (and writes the CSV, if asked for).
    pub fn report(&self, refresh_hz: Option<f64>) {
        let log = &self.log;
        let emulate: Vec<f64> = log.iter().map(|f| ms(f.emulate)).collect();
        let presented: Vec<(&FrameLog, &Presented)> =
            log.iter().filter_map(|f| f.present.as_ref().map(|p| (f, p))).collect();
        let render: Vec<f64> = presented.iter().map(|(f, p)| ms(f.upload + p.render)).collect();
        let interval: Vec<f64> = presented.iter().filter_map(|(_, p)| p.interval.map(ms)).collect();
        let acquire: Vec<f64> = presented.iter().map(|(_, p)| ms(p.acquire)).collect();
        // A frame's own work: emulation, upload and (if shown) render.
        let work: Vec<f64> = log
            .iter()
            .map(|f| ms(f.emulate + f.upload + f.present.map_or(Duration::ZERO, |p| p.render)))
            .collect();
        let line = |name: &str, v: Vec<f64>| match spread(v) {
            Some((p50, p99, max)) => println!("{name:<28} p50 {p50:7.3}  p99 {p99:7.3}  max {max:8.3} ms"),
            None => println!("{name:<28} (none)"),
        };
        println!("selftest: {} machine frames, {} presented", log.len(), presented.len());
        if let Some(hz) = refresh_hz {
            println!("display refresh {hz:.2} Hz");
        }
        line("emulation", emulate);
        line("render-submit (incl upload)", render);
        line("swapchain acquire (wait)", acquire);
        line("present interval", interval.clone());
        let over = |limit: f64| work.iter().filter(|&&w| w > limit).count();
        println!("frames with emulation+render over {FRAME_MS:.1} ms: {}", over(FRAME_MS));
        println!("frames with emulation+render over {TIGHT_MS:.1} ms: {}", over(TIGHT_MS));
        let dropped = log.len() - presented.len();
        let late = interval.iter().filter(|&&i| i > 1.5 * FRAME_MS).count();
        println!(
            "dropped: {dropped} machine frames never presented; {late} present intervals over {:.1} ms",
            1.5 * FRAME_MS
        );
        if let Some(path) = &self.csv {
            if let Err(e) = self.write_csv(path) {
                eprintln!("terse-host: {}: {e}", path.display());
            } else {
                println!("per-frame log: {}", path.display());
            }
        }
    }

    fn write_csv(&self, path: &PathBuf) -> std::io::Result<()> {
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        writeln!(out, "frame,emulate_ms,upload_ms,presented,acquire_ms,render_ms,interval_ms")?;
        for (i, f) in self.log.iter().enumerate() {
            let (shown, acquire, render, interval) = match f.present {
                Some(p) => (1, ms(p.acquire), ms(p.render), p.interval.map_or(String::new(), |d| format!("{:.3}", ms(d)))),
                None => (0, 0.0, 0.0, String::new()),
            };
            writeln!(
                out,
                "{i},{:.3},{:.3},{shown},{acquire:.3},{render:.3},{interval}",
                ms(f.emulate),
                ms(f.upload)
            )?;
        }
        out.flush()
    }
}
