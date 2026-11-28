//! `terse perf log`: a summary of the records in a directory (or one
//! file), per host identity: start-ups, late frames by component and by
//! program, the worst frames, memory trends, warnings and stops.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::judge::{Component, Verdict};
use crate::record::{self, Line};

/// What the lines of one host identity add up to.
#[derive(Default)]
pub struct Summary {
    pub host: String,
    pub env: Option<String>,
    pub devices: BTreeMap<String, String>,
    pub runs: u64,
    pub dates: (Option<String>, Option<String>),
    pub starts: Vec<(f64, String)>,
    pub frames: u64,
    pub late_total: u64,
    pub late: BTreeMap<(String, &'static str), u64>,
    pub by_image: BTreeMap<String, u64>,
    pub worst: Vec<(f64, String)>,
    /// (component, run) → (first seconds, first KiB, last seconds, last KiB, max KiB).
    pub mem: BTreeMap<(String, u64), (u64, u64, u64, u64, u64)>,
    pub blocks: u64,
    pub sheds: u64,
    /// Size changes of the window's surface.
    pub resizes: u64,
    pub warnings: Vec<String>,
    pub stops: Vec<String>,
    /// The host's own components that did not keep up (`fail` lines).
    pub fails: Vec<String>,
    /// Late frames the rule did not count, by reason (`excused` lines).
    pub excused: BTreeMap<String, u64>,
    /// Responses the host was slow to give (`slow` lines, R20), by what:
    /// (count, worst ms).
    pub slow: BTreeMap<String, (u64, f64)>,
    /// Content requests the relays were slow to answer (`relay` lines):
    /// count, worst ms. Not the host's.
    pub relay: (u64, f64),
    pub checks: Vec<String>,
    pub dropped: u64,
    /// Devices whose CPU wait the system did not say (`readings`).
    pub unmeasured: std::collections::BTreeSet<String>,
}

impl Summary {
    pub fn add(&mut self, lines: &[Line]) {
        for l in lines {
            match l {
                Line::Run { date, .. } => {
                    self.runs += 1;
                    if self.dates.0.as_ref().is_none_or(|d| date < d) {
                        self.dates.0 = Some(date.clone());
                    }
                    if self.dates.1.as_ref().is_none_or(|d| date > d) {
                        self.dates.1 = Some(date.clone());
                    }
                }
                Line::Host(i) => self.host = i.line(),
                Line::Env(e) => {
                    self.env = Some(format!("{} {} {}, {} cores, {} GiB, renderer {}, {}", e.os, e.os_version, e.arch, e.cores, e.ram_gib, e.renderer, e.cpu));
                }
                Line::Device { component, slots, identity } => {
                    let s: Vec<String> = slots.iter().map(u8::to_string).collect();
                    self.devices.insert(component.name(), format!("slots {}: {}", s.join(","), identity.line()));
                }
                Line::Readings { component, readings } => {
                    if component.is_device() && !readings.wait {
                        self.unmeasured.insert(component.name());
                    }
                }
                Line::Start { ms, cold } => self.starts.push((*ms, cold.clone())),
                Line::Late { frame, ms, verdict, component, own_ms, cpu_ms, blocked_ms, starved_ms, image } => {
                    *self.late.entry((component.name(), verdict.name())).or_default() += 1;
                    if *verdict == Verdict::Blamed {
                        *self.by_image.entry(image.clone()).or_default() += 1;
                    }
                    self.worst.push((
                        *ms,
                        format!(
                            "frame {frame}: {ms:.1} ms, {} {component} (own {own_ms:.1}: cpu {}, blocked {}; starved {starved_ms:.1}), program {}",
                            verdict.name(),
                            cpu_ms.map_or("-".into(), |v| format!("{v:.1}")),
                            blocked_ms.map_or("-".into(), |v| format!("{v:.1}")),
                            if image.is_empty() { "-" } else { image }
                        ),
                    ));
                }
                Line::Block { .. } => self.blocks += 1,
                Line::Shed { .. } => self.sheds += 1,
                Line::Resize { .. } => self.resizes += 1,
                Line::Warn { frame, component, count } => self.warnings.push(format!("frame {frame}: {component} late {count} times")),
                Line::Stop { frame, component, machine, image, saved } => {
                    self.stops.push(format!("frame {frame}: {component}, machine {machine} (program {}), saved {saved}", if image.is_empty() { "-" } else { image }))
                }
                Line::Mem { seconds, component, rss_kib } => {
                    let e = self.mem.entry((component.name(), self.runs)).or_insert((*seconds, *rss_kib, *seconds, *rss_kib, *rss_kib));
                    e.2 = *seconds;
                    e.3 = *rss_kib;
                    e.4 = e.4.max(*rss_kib);
                }
                Line::Check { component, ms, result } => self.checks.push(format!("{component} {ms:.1} ms: {result}")),
                Line::End { frames, late, .. } => {
                    self.frames += frames;
                    self.late_total += late;
                }
                Line::Drop(n) => self.dropped += n,
                Line::Other(_) => {
                    if let Some((frame, component, count)) = l.as_fail() {
                        self.fails.push(format!("frame {frame}: the host's own {component}, {count} late frames with nothing left to shed"));
                    } else if let Some((_, why)) = l.as_excused() {
                        *self.excused.entry(why.to_string()).or_default() += 1;
                    } else if let Some((_, ms, what)) = l.as_slow() {
                        let e = self.slow.entry(what.to_string()).or_default();
                        e.0 += 1;
                        e.1 = e.1.max(ms);
                    } else if let Some((_, ms)) = l.as_relay() {
                        self.relay.0 += 1;
                        self.relay.1 = self.relay.1.max(ms);
                    }
                }
                Line::Format(_) => {}
            }
        }
    }

    pub fn report(&self) -> String {
        let mut o = String::new();
        let _ = writeln!(o, "{}", if self.host.is_empty() { "(no host line)" } else { &self.host });
        let dates = match &self.dates {
            (Some(a), Some(b)) if a != b => format!(", {a} to {b}"),
            (Some(a), _) => format!(", {a}"),
            _ => String::new(),
        };
        let _ = writeln!(o, "  runs: {}{dates}", self.runs);
        if let Some(e) = &self.env {
            let _ = writeln!(o, "  computer: {e}");
        }
        for (c, d) in &self.devices {
            let _ = writeln!(o, "  device {c}: {d}");
        }
        if !self.unmeasured.is_empty() {
            let v: Vec<&str> = self.unmeasured.iter().map(String::as_str).collect();
            let _ = writeln!(o, "  no CPU-wait readings for {}: blamed by their waits alone, never stopped", v.join(", "));
        }
        if !self.starts.is_empty() {
            let mut v: Vec<f64> = self.starts.iter().map(|s| s.0).collect();
            v.sort_by(f64::total_cmp);
            let cold: Vec<String> = self.starts.iter().filter(|s| s.1 == "build" || s.1 == "boot").map(|s| format!("{:.0} ms ({})", s.0, s.1)).collect();
            let _ = writeln!(
                o,
                "  start-up to the first frame: {} runs, median {:.0} ms, worst {:.0} ms{}",
                v.len(),
                v[v.len() / 2],
                v[v.len() - 1],
                if cold.is_empty() { String::new() } else { format!("; cold: {}", cold.join(", ")) }
            );
        }
        let lates: u64 = self.late.values().sum();
        let _ = writeln!(o, "  frames: {} in runs that ended cleanly, late {} ({} recorded)", self.frames, self.late_total, lates);
        if !self.late.is_empty() {
            let mut by: BTreeMap<&str, BTreeMap<&str, u64>> = BTreeMap::new();
            for ((c, v), n) in &self.late {
                *by.entry(c).or_default().entry(v).or_default() += n;
            }
            for (c, v) in by {
                let parts: Vec<String> = v.iter().map(|(k, n)| format!("{k} {n}")).collect();
                let _ = writeln!(o, "    {c}: {}", parts.join(", "));
            }
        }
        if !self.by_image.is_empty() {
            let _ = writeln!(o, "  late frames blamed, by program:");
            for (i, n) in &self.by_image {
                let _ = writeln!(o, "    {}: {n}", if i.is_empty() { "-" } else { i });
            }
        }
        if !self.worst.is_empty() {
            let mut w = self.worst.clone();
            w.sort_by(|a, b| b.0.total_cmp(&a.0));
            let _ = writeln!(o, "  worst frames:");
            for (_, l) in w.iter().take(5) {
                let _ = writeln!(o, "    {l}");
            }
        }
        if self.blocks > 0 || self.sheds > 0 {
            let _ = writeln!(o, "  blockages (waits of 100 ms or more): {}; the look shed {} times", self.blocks, self.sheds);
        }
        if self.resizes > 0 {
            let _ = writeln!(o, "  the window changed size {} times (late frames just after: resize, not counted)", self.resizes);
        }
        let mut grew: Vec<(f64, String)> = Vec::new();
        for ((c, _), (s0, k0, s1, k1, max)) in &self.mem {
            if s1 > s0 {
                let per_min = (*k1 as f64 - *k0 as f64) / 1024.0 / ((*s1 - *s0) as f64 / 60.0);
                grew.push((per_min, format!("    {c}: {:.1} to {:.1} MiB over {} s ({per_min:+.2} MiB a minute; at most {:.1} MiB)", *k0 as f64 / 1024.0, *k1 as f64 / 1024.0, s1 - s0, *max as f64 / 1024.0)));
            }
        }
        if !grew.is_empty() {
            grew.sort_by(|a, b| b.0.total_cmp(&a.0));
            let _ = writeln!(o, "  memory (the runs that grew most first):");
            for (_, l) in grew.iter().take(8) {
                let _ = writeln!(o, "{l}");
            }
        }
        for c in &self.checks {
            let _ = writeln!(o, "  hardware check: {c}");
        }
        for w in &self.warnings {
            let _ = writeln!(o, "  warning: {w}");
        }
        for s in &self.stops {
            let _ = writeln!(o, "  stopped: {s}");
        }
        for s in &self.fails {
            let _ = writeln!(o, "  did not keep up: {s}");
        }
        if !self.excused.is_empty() {
            let e: Vec<String> = self.excused.iter().map(|(why, n)| format!("{why} {n}")).collect();
            let _ = writeln!(o, "  late frames not counted (excused): {}", e.join(", "));
        }
        for (what, (n, worst)) in &self.slow {
            let kind = match what.as_str() {
                "store" => "a content request answered from the host's own store",
                "page" => "a page shown after its submit (layout and figures)",
                _ => what.as_str(),
            };
            let _ = writeln!(o, "  slow responses of the host (R20): {kind}: {n}, the slowest {worst:.1} ms");
        }
        if self.relay.0 > 0 {
            let _ = writeln!(o, "  content requests the relays took a second or more to answer (the network's, not a fault): {}, the slowest {:.1} ms", self.relay.0, self.relay.1);
        }
        if self.dropped > 0 {
            let _ = writeln!(o, "  lines lost (the writer fell behind): {}", self.dropped);
        }
        o
    }
}

/// Summarises `path`: a record file, or every record in a directory (a
/// file and its rotated `.1` together).
pub fn summarise(path: &Path) -> Result<String, String> {
    let mut groups: BTreeMap<String, Vec<std::path::PathBuf>> = BTreeMap::new();
    if path.is_dir() {
        for e in std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))?.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if let Some(stem) = n.strip_suffix(".log.1").or_else(|| n.strip_suffix(".log")) {
                groups.entry(stem.to_string()).or_default().push(e.path());
            }
        }
    } else {
        groups.insert(path.display().to_string(), vec![path.to_path_buf()]);
    }
    if groups.is_empty() {
        return Ok(format!("no records in {}\n", path.display()));
    }
    let mut out = String::new();
    for (_, mut files) in groups {
        // The rotated file holds the older lines.
        files.sort_by_key(|p| !p.to_string_lossy().ends_with(".1"));
        let mut s = Summary::default();
        for f in files {
            let text = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
            s.add(&record::parse(&text));
        }
        out.push_str(&s.report());
    }
    Ok(out)
}

/// The components a summary names, for tests.
pub fn components(lines: &[Line]) -> Vec<Component> {
    let mut v: Vec<Component> = lines
        .iter()
        .filter_map(|l| match l {
            Line::Late { component, .. } | Line::Warn { component, .. } | Line::Stop { component, .. } => Some(*component),
            _ => None,
        })
        .collect();
    v.sort();
    v.dedup();
    v
}
