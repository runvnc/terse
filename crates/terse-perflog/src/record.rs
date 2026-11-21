//! The record file (`RECORD.md`): line-oriented text, append-only, one
//! file per host identity in the record directory, bounded and rotated,
//! written by a thread of its own so that the frame loop never waits on
//! it.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::env::Environment;
use crate::identity::Identity;
use crate::judge::{Component, Late, Verdict};

/// The format's version: the first line of every file is `terse-perf 1`.
pub const FORMAT: u32 = 1;
/// A file is rotated (renamed `.1`, replacing the last one) when it
/// passes this size, so one host identity takes at most twice it.
pub const FILE_CAP: u64 = 256 * 1024;
/// The most record files a directory keeps (the oldest go first).
pub const MAX_FILES: usize = 16;
/// The most `late` lines one run writes; later late frames are only
/// counted (in `end`).
pub const MAX_LATE_LINES: u64 = 2000;
/// Lines waiting for the writer; when they are full a line is dropped
/// (and counted in a `drop` line) rather than wait.
pub const QUEUE: usize = 1024;
/// Memory is sampled every this many frames (10 s at 60 a second).
pub const MEM_EVERY: u64 = 600;
/// A single wait on a component this long or longer is a blockage.
pub const BLOCK_NS: u64 = 100_000_000;

/// One line of the record.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    /// `terse-perf <version>`: the first line of every file.
    Format(u32),
    /// `run <date> <mode>`: a run of the host begins (the UTC date; mode
    /// `window`, `realtime` or `headless`).
    Run { date: String, mode: String },
    /// `host <name> <version> <build>`.
    Host(Identity),
    /// `env <os> <os-version> <arch> <cores> <ram-gib> <renderer> <cpu>`.
    Env(Environment),
    /// `device <component> <slots> <name> <version> <build>`.
    Device { component: Component, slots: Vec<u8>, identity: Identity },
    /// `start <ms> <cold>`: from the process's start to its first frame
    /// presented; cold is `build` (the first run of this build), `boot`
    /// (the first since the computer started), `warm` or `-`.
    Start { ms: f64, cold: String },
    /// `late <frame> <ms> <verdict> <component> <own-ms> <cpu-ms>
    /// <blocked-ms> <starved-ms> <image>`.
    Late { frame: u64, ms: f64, verdict: Verdict, component: Component, own_ms: f64, cpu_ms: f64, blocked_ms: f64, starved_ms: f64, image: String },
    /// `block <frame> <ms> <component> <what>`: one wait of 100 ms or more
    /// (`what`: `frame` work, `open` a machine, `service`, `state`).
    Block { frame: u64, ms: f64, component: Component, what: String },
    /// `shed <frame> <level> <why>`: the host shed (or took back) optional
    /// work (`why`: `late`, `recover`, `check`).
    Shed { frame: u64, level: u32, why: String },
    /// `warn <frame> <component> <count>`.
    Warn { frame: u64, component: Component, count: u32 },
    /// `stop <frame> <component> <machine> <image> <saved>`: the host
    /// stopped a machine (`machine`: its handle, 0 for the shell or a
    /// machine alone; `saved`: `slot<n>`, `file`, `none`).
    Stop { frame: u64, component: Component, machine: u32, image: String, saved: String },
    /// `mem <seconds> <component> <rss-kib>`.
    Mem { seconds: u64, component: Component, rss_kib: u64 },
    /// `check <component> <ms> <result>`: the start-up hardware check of a
    /// device's worst legal frame (`result`: `ok`, `shed`, `absent`).
    Check { component: Component, ms: f64, result: String },
    /// `end <frames> <late> <seconds>`.
    End { frames: u64, late: u64, seconds: u64 },
    /// `drop <lines>`: lines lost because the writer fell behind.
    Drop(u64),
    /// A line of a type this reader does not know (kept, not understood).
    Other(String),
}

/// A string as one field: bytes outside `!`..`~`, and `%`, as `%XX`;
/// empty as `-`.
pub fn escape(s: &str) -> String {
    if s.is_empty() {
        return "-".into();
    }
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_graphic() && b != b'%' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The inverse of [`escape`].
pub fn unescape(s: &str) -> String {
    if s == "-" {
        return String::new();
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn ms(ns: u64) -> f64 {
    (ns as f64 / 100_000.0).round() / 10.0
}

/// The first 16 hex digits of an image ID, or `-`.
pub fn image_field(id: Option<&[u8; 32]>) -> String {
    match id {
        Some(id) => id[..8].iter().map(|b| format!("{b:02x}")).collect(),
        None => "-".into(),
    }
}

impl Line {
    /// The line for a late frame.
    pub fn late(l: &Late) -> Line {
        let p = &l.top;
        Line::Late {
            frame: l.frame,
            ms: ms(l.wall_ns),
            verdict: l.verdict,
            component: p.component,
            own_ms: ms(p.own()),
            cpu_ms: ms(p.cpu()),
            blocked_ms: ms(p.blocked()),
            starved_ms: ms(p.starved()),
            image: image_field(p.image.as_ref()),
        }
    }

    pub fn encode(&self) -> String {
        let id = |i: &Identity| format!("{} {} {}", escape(&i.name), escape(&i.version), escape(&i.build));
        match self {
            Line::Format(v) => format!("terse-perf {v}"),
            Line::Run { date, mode } => format!("run {} {}", escape(date), escape(mode)),
            Line::Host(i) => format!("host {}", id(i)),
            Line::Env(e) => format!(
                "env {} {} {} {} {} {} {}",
                escape(&e.os),
                escape(&e.os_version),
                escape(&e.arch),
                e.cores,
                e.ram_gib,
                escape(&e.renderer),
                escape(&e.cpu)
            ),
            Line::Device { component, slots, identity } => {
                let s: Vec<String> = slots.iter().map(u8::to_string).collect();
                format!("device {component} {} {}", if s.is_empty() { "-".into() } else { s.join(",") }, id(identity))
            }
            Line::Start { ms, cold } => format!("start {ms:.1} {}", escape(cold)),
            Line::Late { frame, ms, verdict, component, own_ms, cpu_ms, blocked_ms, starved_ms, image } => {
                format!("late {frame} {ms:.1} {} {component} {own_ms:.1} {cpu_ms:.1} {blocked_ms:.1} {starved_ms:.1} {}", verdict.name(), escape(image))
            }
            Line::Block { frame, ms, component, what } => format!("block {frame} {ms:.1} {component} {}", escape(what)),
            Line::Shed { frame, level, why } => format!("shed {frame} {level} {}", escape(why)),
            Line::Warn { frame, component, count } => format!("warn {frame} {component} {count}"),
            Line::Stop { frame, component, machine, image, saved } => format!("stop {frame} {component} {machine} {} {}", escape(image), escape(saved)),
            Line::Mem { seconds, component, rss_kib } => format!("mem {seconds} {component} {rss_kib}"),
            Line::Check { component, ms, result } => format!("check {component} {ms:.1} {}", escape(result)),
            Line::End { frames, late, seconds } => format!("end {frames} {late} {seconds}"),
            Line::Drop(n) => format!("drop {n}"),
            Line::Other(s) => s.clone(),
        }
    }

    /// Parses one line; `None` for a malformed line of a known type (a
    /// reader skips it), [`Line::Other`] for an unknown type.
    pub fn parse(line: &str) -> Option<Line> {
        let f: Vec<&str> = line.split(' ').collect();
        let n = |i: usize| -> Option<u64> { f.get(i)?.parse().ok() };
        let x = |i: usize| -> Option<f64> { f.get(i)?.parse().ok() };
        let s = |i: usize| -> Option<String> { Some(unescape(f.get(i)?)) };
        let c = |i: usize| -> Option<Component> { Component::parse(f.get(i)?) };
        let id = |i: usize| -> Option<Identity> { Some(Identity { name: s(i)?, version: s(i + 1)?, build: s(i + 2)? }) };
        let want = |k: usize| f.len() >= k;
        Some(match f[0] {
            "terse-perf" if want(2) => Line::Format(n(1)? as u32),
            "run" if want(3) => Line::Run { date: s(1)?, mode: s(2)? },
            "host" if want(4) => Line::Host(id(1)?),
            "env" if want(8) => Line::Env(Environment {
                os: s(1)?,
                os_version: s(2)?,
                arch: s(3)?,
                cores: n(4)? as u32,
                ram_gib: n(5)? as u32,
                renderer: s(6)?,
                cpu: s(7)?,
            }),
            "device" if want(6) => Line::Device {
                component: c(1)?,
                slots: if f[2] == "-" { Vec::new() } else { f[2].split(',').map(|v| v.parse().ok()).collect::<Option<_>>()? },
                identity: id(3)?,
            },
            "start" if want(3) => Line::Start { ms: x(1)?, cold: s(2)? },
            "late" if want(10) => Line::Late {
                frame: n(1)?,
                ms: x(2)?,
                verdict: Verdict::parse(f[3])?,
                component: c(4)?,
                own_ms: x(5)?,
                cpu_ms: x(6)?,
                blocked_ms: x(7)?,
                starved_ms: x(8)?,
                image: s(9)?,
            },
            "block" if want(5) => Line::Block { frame: n(1)?, ms: x(2)?, component: c(3)?, what: s(4)? },
            "shed" if want(4) => Line::Shed { frame: n(1)?, level: n(2)? as u32, why: s(3)? },
            "warn" if want(4) => Line::Warn { frame: n(1)?, component: c(2)?, count: n(3)? as u32 },
            "stop" if want(6) => Line::Stop { frame: n(1)?, component: c(2)?, machine: n(3)? as u32, image: s(4)?, saved: s(5)? },
            "mem" if want(4) => Line::Mem { seconds: n(1)?, component: c(2)?, rss_kib: n(3)? },
            "check" if want(4) => Line::Check { component: c(1)?, ms: x(2)?, result: s(3)? },
            "end" if want(4) => Line::End { frames: n(1)?, late: n(2)?, seconds: n(3)? },
            "drop" if want(2) => Line::Drop(n(1)?),
            "terse-perf" | "run" | "host" | "env" | "device" | "start" | "late" | "block" | "shed" | "warn" | "stop" | "mem" | "check" | "end" | "drop" => return None,
            t if !t.is_empty() && !t.starts_with('#') => Line::Other(line.to_string()),
            _ => return None,
        })
    }

    /// Whether the line describes the run (repeated at the head of a
    /// rotated file, so that every file can be read alone).
    fn is_header(&self) -> bool {
        matches!(self, Line::Run { .. } | Line::Host(_) | Line::Env(_) | Line::Device { .. } | Line::Start { .. })
    }
}

/// Every line of a record's text that parses (comments, blank and
/// malformed lines skipped).
pub fn parse(text: &str) -> Vec<Line> {
    text.lines().filter_map(|l| Line::parse(l.trim_end_matches('\r'))).collect()
}

/// The default record directory: `$XDG_DATA_HOME/terse/perf`, else
/// `~/.local/share/terse/perf`.
pub fn default_dir() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".local/share"),
    };
    Some(base.join("terse").join("perf"))
}

/// The record file of `host` in `dir`: `<name>-<build hex>.log`.
pub fn file_of(dir: &Path, host: &Identity) -> PathBuf {
    let clean = |s: &str| s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect::<String>();
    let build = host.build.rsplit(':').next().unwrap_or("");
    dir.join(format!("{}-{}.log", clean(&host.name), clean(if build.is_empty() { "unknown" } else { build })))
}

/// Today's date (UTC) as `YYYY-MM-DD`.
pub fn utc_date() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

enum Msg {
    Line(String, bool),
    Flush(SyncSender<()>),
}

/// Writes a record off the frame loop. Every method returns at once: a
/// line that finds the queue full is dropped and counted.
pub struct Recorder {
    tx: Option<SyncSender<Msg>>,
    thread: Option<JoinHandle<()>>,
    dropped: Arc<AtomicU64>,
    late_lines: AtomicU64,
    pub path: PathBuf,
    /// Whether this host identity had no record yet (its first run).
    pub first: bool,
    /// The writer's first error, if any (the record is then lost, the run
    /// goes on).
    pub error: Arc<Mutex<Option<String>>>,
}

impl Recorder {
    /// Starts the writer of `host`'s record in `dir` (created if need be).
    pub fn open(dir: &Path, host: &Identity) -> Recorder {
        let path = file_of(dir, host);
        let first = !path.exists() && !path.with_extension("log.1").exists();
        let (tx, rx) = sync_channel(QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));
        let error = Arc::new(Mutex::new(None));
        let (d, p, e, dir) = (dropped.clone(), path.clone(), error.clone(), dir.to_path_buf());
        let thread = std::thread::Builder::new().name("terse-perf-record".into()).spawn(move || {
            if let Err(err) = write_loop(&dir, &p, rx, &d) {
                *e.lock().unwrap() = Some(err.to_string());
            }
        });
        Recorder { tx: Some(tx), thread: thread.ok(), dropped, late_lines: AtomicU64::new(0), path, first, error }
    }

    /// Queues a line (never waits). Late lines past [`MAX_LATE_LINES`]
    /// are not written.
    pub fn add(&self, line: Line) {
        if matches!(line, Line::Late { .. }) && self.late_lines.fetch_add(1, Ordering::Relaxed) >= MAX_LATE_LINES {
            return;
        }
        let header = line.is_header();
        let Some(tx) = &self.tx else { return };
        match tx.try_send(Msg::Line(line.encode(), header)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// Waits (at most a second) until everything queued is written.
    pub fn flush(&self) {
        let Some(tx) = &self.tx else { return };
        let (done, wait) = sync_channel(1);
        if tx.send(Msg::Flush(done)).is_ok() {
            let _ = wait.recv_timeout(Duration::from_secs(1));
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.flush();
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Removes the oldest record files beyond [`MAX_FILES`].
fn prune(dir: &Path, keep: &Path) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.ends_with(".log") || n.ends_with(".log.1")
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= MAX_FILES {
        return;
    }
    files.sort();
    let excess = files.len() - MAX_FILES;
    for (_, p) in files.into_iter().filter(|(_, p)| p != keep).take(excess) {
        let _ = fs::remove_file(p);
    }
}

fn write_loop(dir: &Path, path: &Path, rx: Receiver<Msg>, dropped: &AtomicU64) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let open = |path: &Path| -> std::io::Result<(BufWriter<File>, u64)> {
        let f = OpenOptions::new().create(true).append(true).open(path)?;
        let size = f.metadata()?.len();
        Ok((BufWriter::new(f), size))
    };
    let (mut out, mut size) = open(path)?;
    if size == 0 {
        let l = format!("{}\n", Line::Format(FORMAT).encode());
        out.write_all(l.as_bytes())?;
        size += l.len() as u64;
    }
    prune(dir, path);
    let mut header: Vec<String> = Vec::new();
    let mut reported = 0;
    loop {
        let Ok(first) = rx.recv() else { break };
        let mut batch = vec![first];
        while let Ok(m) = rx.try_recv() {
            batch.push(m);
        }
        let mut flushes = Vec::new();
        for m in batch {
            match m {
                Msg::Line(l, is_header) => {
                    if is_header {
                        header.push(l.clone());
                    }
                    if size + l.len() as u64 + 1 > FILE_CAP {
                        out.flush()?;
                        drop(out);
                        fs::rename(path, path.with_extension("log.1"))?;
                        (out, size) = open(path)?;
                        let mut head = format!("{}\n", Line::Format(FORMAT).encode());
                        for h in &header {
                            head.push_str(h);
                            head.push('\n');
                        }
                        out.write_all(head.as_bytes())?;
                        size += head.len() as u64;
                    }
                    out.write_all(l.as_bytes())?;
                    out.write_all(b"\n")?;
                    size += l.len() as u64 + 1;
                }
                Msg::Flush(done) => flushes.push(done),
            }
        }
        let lost = dropped.load(Ordering::Relaxed);
        if lost > reported {
            let l = format!("{}\n", Line::Drop(lost - reported).encode());
            out.write_all(l.as_bytes())?;
            size += l.len() as u64;
            reported = lost;
        }
        out.flush()?;
        for f in flushes {
            let _ = f.send(());
        }
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judge::Part;

    #[test]
    fn escaping() {
        for s in ["", "Intel(R) Core(TM) i7-10750H CPU @ 2.60GHz", "a%b", "naïve", "x\ny"] {
            let e = escape(s);
            assert!(!e.contains(' ') && !e.contains('\n') && !e.is_empty(), "{e}");
            assert_eq!(unescape(&e), s);
        }
        assert_eq!(escape("a b%"), "a%20b%25");
    }

    #[test]
    fn every_line_round_trips() {
        let i = Identity { name: "terse-dev-terminal (Rust)".into(), version: "0.1.0".into(), build: "b3:0123456789abcdef".into() };
        let late = Late {
            frame: 1200,
            wall_ns: 52_340_000,
            verdict: Verdict::Blamed,
            top: Part { component: Component::Device(7), wall_ns: 48_000_000, run_ns: Some(1_000_000), starved_ns: Some(500_000), image: Some([0xab; 32]) },
        };
        let lines = vec![
            Line::Format(1),
            Line::Run { date: "2026-11-21".into(), mode: "window".into() },
            Line::Host(i.clone()),
            Line::Env(Environment::detect("software")),
            Line::Device { component: Component::Device(2), slots: vec![2, 10], identity: i },
            Line::Start { ms: 84.2, cold: "build".into() },
            Line::late(&late),
            Line::Block { frame: 3, ms: 812.0, component: Component::Device(4), what: "service".into() },
            Line::Shed { frame: 9, level: 2, why: "late".into() },
            Line::Warn { frame: 1300, component: Component::Device(7), count: 3 },
            Line::Stop { frame: 1500, component: Component::Device(7), machine: 2, image: "abababababababab".into(), saved: "slot17".into() },
            Line::Mem { seconds: 10, component: Component::Core, rss_kib: 51_200 },
            Line::Check { component: Component::Device(3), ms: 6.5, result: "ok".into() },
            Line::End { frames: 3600, late: 12, seconds: 60 },
            Line::Drop(4),
            Line::Other("future 1 2 3".into()),
        ];
        for l in &lines {
            let e = l.encode();
            assert!(e.len() < 256 && !e.contains('\n'), "{e}");
            assert_eq!(Line::parse(&e).as_ref(), Some(l), "{e}");
        }
        assert_eq!(
            Line::late(&late).encode(),
            "late 1200 52.3 blamed dev7 47.5 1.0 46.5 0.5 abababababababab"
        );
        assert_eq!(Line::parse("late 1 2"), None);
        assert_eq!(Line::parse("# a comment"), None);
    }

    #[test]
    fn writes_rotates_and_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let host = Identity { name: "terse-bus".into(), version: "0.1.0".into(), build: "b3:00112233aabbccdd".into() };
        let r = Recorder::open(dir.path(), &host);
        assert!(r.first);
        assert_eq!(r.path, dir.path().join("terse-bus-00112233aabbccdd.log"));
        r.add(Line::Run { date: utc_date(), mode: "headless".into() });
        r.add(Line::Host(host.clone()));
        for n in 0..30_000u64 {
            r.add(Line::Mem { seconds: n, component: Component::Device(3), rss_kib: 1000 + n });
            if n % 500 == 0 {
                r.flush();
            }
        }
        drop(r);
        let a = fs::read_to_string(dir.path().join("terse-bus-00112233aabbccdd.log")).unwrap();
        let b = fs::read_to_string(dir.path().join("terse-bus-00112233aabbccdd.log.1")).unwrap();
        assert!(a.len() as u64 <= FILE_CAP && b.len() as u64 <= FILE_CAP, "{} {}", a.len(), b.len());
        // Each file stands alone: its format line, then the run's header.
        for t in [&a, &b] {
            let l = parse(t);
            assert_eq!(l[0], Line::Format(1));
            assert!(matches!(l[1], Line::Run { .. }) && l[2] == Line::Host(host.clone()), "{:?}", &l[..3]);
        }
        let last = parse(&a).into_iter().rev().find_map(|l| if let Line::Mem { seconds, .. } = l { Some(seconds) } else { None });
        assert_eq!(last, Some(29_999));
        let again = Recorder::open(dir.path(), &host);
        assert!(!again.first);
    }

    #[test]
    fn dates() {
        let d = utc_date();
        assert_eq!(d.len(), 10);
        assert!(d.as_str() >= "2026-01-01", "{d}");
    }
}
