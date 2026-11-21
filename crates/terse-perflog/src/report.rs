//! The performance report (`REPORT.md`): a small, public summary of one
//! host identity's record over a period, plus the results of a `terse
//! perf` run if one was made, that a user may choose to publish as a
//! document of the `reports` format. Text, one record per line, in the
//! record's own line syntax (`RECORD.md` section 2), so that a person can
//! read it and a Terse32 program can parse it within its limits.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::env::Environment;
use crate::identity::Identity;
use crate::judge::{Component, Verdict};
use crate::record::{Line, escape, unescape};

/// The format's version: the first line is `terse-report 1`.
pub const FORMAT: u32 = 1;
/// The document format's name (`spec/content.md` §content.documents); its
/// owner key is all zero: the format of a built-in program.
pub const FORMAT_NAME: &str = "reports";
/// The document header's length: `TDOC`, version, owner key, name length,
/// name.
pub const HEAD: usize = 38 + FORMAT_NAME.len();
/// The hard cap on a whole document, header included.
pub const MAX_DOCUMENT: usize = 8192;
/// What a report should stay within (a target, not a rule).
pub const TARGET: usize = 4096;
/// The longest line, as in the record.
pub const MAX_LINE: usize = 255;
/// The most lines of the types that repeat.
pub const MAX_DEVICES: usize = 16;
pub const MAX_COMPONENT_LINES: usize = 20;
pub const MAX_BENCH: usize = 64;
pub const MAX_PROGRAMS: usize = 5;
/// The longest benchmark row id.
pub const MAX_BENCH_ID: usize = 24;
/// The longest name (unescaped) in a `host`, `device` or `perf` line.
pub const MAX_NAME: usize = 64;

/// The start-up times of the period's runs.
#[derive(Clone, Debug, PartialEq)]
pub struct Start {
    pub count: u32,
    pub median_ms: f64,
    pub worst_ms: f64,
    /// Starts that were cold (`build` or `boot`), and the worst of them.
    pub cold: u32,
    pub cold_worst_ms: Option<f64>,
}

/// The outcome of one tier of a benchmark row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Pass,
    Fail,
    /// The row has no such tier.
    None,
}

impl Tier {
    fn word(self, pass: &'static str, fail: &'static str) -> &'static str {
        match self {
            Tier::Pass => pass,
            Tier::Fail => fail,
            Tier::None => "-",
        }
    }

    fn parse(s: &str, pass: &str, fail: &str) -> Option<Tier> {
        if s == pass {
            Some(Tier::Pass)
        } else if s == fail {
            Some(Tier::Fail)
        } else if s == "-" {
            Some(Tier::None)
        } else {
            None
        }
    }
}

/// One row of a `terse perf` run.
#[derive(Clone, Debug, PartialEq)]
pub struct Bench {
    pub id: String,
    /// The measured value (none when the row measures no number).
    pub value: Option<f64>,
    /// Its unit (`frames`, `ms`, `trips`, …), empty for none.
    pub unit: String,
    pub requirement: Tier,
    pub ceiling: Tier,
    /// `Pass`: the target was met; `Fail`: missed.
    pub target: Tier,
}

/// A component's memory over the period.
#[derive(Clone, Debug, PartialEq)]
pub struct Mem {
    pub component: Component,
    /// The most resident memory sampled, MiB.
    pub peak_mib: f64,
    /// The fastest growth over one run of a minute or more, MiB a minute.
    pub growth: Option<f64>,
}

/// A performance report.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// The UTC dates of the first and the last day covered.
    pub from: String,
    pub to: String,
    pub host: Identity,
    pub env: Environment,
    pub devices: Vec<(Component, Vec<u8>, Identity)>,
    /// Runs begun in the period, of them ended cleanly, their frames and
    /// late frames.
    pub runs: u32,
    pub ended: u32,
    pub frames: u32,
    pub late_frames: u32,
    pub start: Option<Start>,
    /// Late frames recorded, by component: blamed, starved, memory, spread.
    pub late: Vec<(Component, [u32; 4])>,
    pub warn: Vec<(Component, u32)>,
    pub halt: Vec<(Component, u32)>,
    /// Shed events: for late frames, by the hardware check.
    pub shed: Option<(u32, u32)>,
    pub mem: Vec<Mem>,
    pub check: Vec<(Component, f64, String)>,
    /// The `terse perf` run: its identity and date.
    pub perf: Option<(Identity, String)>,
    pub bench: Vec<Bench>,
    /// Opt-in only: the programs most often working when a frame was
    /// blamed (first 16 hex digits of their image IDs), and how often.
    pub programs: Vec<(String, u32)>,
}

/// Why a document is not a valid report: a short code (the vectors'
/// `reason`) and the line it was found on (1 is the first line after
/// the header; 0 for the document as a whole).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reject {
    pub code: &'static str,
    pub line: usize,
}

impl std::fmt::Display for Reject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line == 0 { write!(f, "{}", self.code) } else { write!(f, "{} (line {})", self.code, self.line) }
    }
}

fn reject(code: &'static str, line: usize) -> Reject {
    Reject { code, line }
}

/// The known line types in the order a report gives them; the number is
/// the type's rank (a line may not follow one of a higher rank).
const ORDER: [&str; 17] =
    ["terse-report", "period", "host", "env", "device", "runs", "start", "late", "warn", "halt", "shed", "mem", "check", "perf", "bench", "program", "end"];

fn host_component(c: Component) -> bool {
    matches!(c, Component::Core | Component::Compose | Component::Present | Component::Outside)
}

fn ms(v: f64) -> String {
    format!("{v:.1}")
}

/// A benchmark value: an integer, or up to three decimals without
/// trailing zeros.
pub fn number(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" { "0".into() } else { s.to_string() }
}

fn ident(i: &Identity) -> String {
    format!("{} {} {}", escape(&i.name), escape(&i.version), escape(&i.build))
}

impl Report {
    /// The report's text (without the document header).
    pub fn text(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        lines.push(format!("terse-report {FORMAT}"));
        lines.push(format!("period {} {}", self.from, self.to));
        lines.push(format!("host {}", ident(&self.host)));
        lines.push(Line::Env(self.env.clone()).encode());
        for (c, slots, id) in &self.devices {
            lines.push(Line::Device { component: *c, slots: slots.clone(), identity: id.clone() }.encode());
        }
        lines.push(format!("runs {} {} {} {}", self.runs, self.ended, self.frames, self.late_frames));
        if let Some(s) = &self.start {
            let cw = s.cold_worst_ms.map_or("-".into(), ms);
            lines.push(format!("start {} {} {} {} {cw}", s.count, ms(s.median_ms), ms(s.worst_ms), s.cold));
        }
        for (c, v) in &self.late {
            lines.push(format!("late {c} {} {} {} {}", v[0], v[1], v[2], v[3]));
        }
        for (c, n) in &self.warn {
            lines.push(format!("warn {c} {n}"));
        }
        for (c, n) in &self.halt {
            lines.push(format!("halt {c} {n}"));
        }
        if let Some((late, check)) = self.shed {
            lines.push(format!("shed {late} {check}"));
        }
        for m in &self.mem {
            lines.push(format!("mem {} {:.1} {}", m.component, m.peak_mib, m.growth.map_or("-".into(), |g| format!("{g:.2}"))));
        }
        for (c, v, r) in &self.check {
            lines.push(format!("check {c} {} {}", ms(*v), escape(r)));
        }
        if let Some((id, date)) = &self.perf {
            lines.push(format!("perf {} {date}", ident(id)));
        }
        for b in &self.bench {
            lines.push(format!(
                "bench {} {} {} {} {} {}",
                escape(&b.id),
                b.value.map_or("-".into(), number),
                escape(&b.unit),
                b.requirement.word("pass", "fail"),
                b.ceiling.word("pass", "fail"),
                b.target.word("met", "missed")
            ));
        }
        for (image, n) in &self.programs {
            lines.push(format!("program {image} {n}"));
        }
        let n = lines.len();
        lines.push(format!("end {n}"));
        let mut out = lines.join("\n");
        out.push('\n');
        out
    }

    /// The whole document: the header of the `reports` format, then the
    /// text.
    pub fn document(&self) -> Vec<u8> {
        let mut d = header();
        d.extend(self.text().into_bytes());
        d
    }

    /// The title of the entry it is published as.
    pub fn title(&self) -> String {
        let t = format!("Performance report: {} {}, {} to {}", self.host.name, self.host.version, self.from, self.to);
        // Titles are at most 120 bytes; cut at a character boundary.
        let mut end = t.len().min(120);
        while !t.is_char_boundary(end) {
            end -= 1;
        }
        t[..end].to_string()
    }
}

/// The document header of the `reports` format.
pub fn header() -> Vec<u8> {
    let mut h = b"TDOC\0".to_vec();
    h.extend([0u8; 32]);
    h.push(FORMAT_NAME.len() as u8);
    h.extend(FORMAT_NAME.as_bytes());
    h
}

/// Parses a whole document (header and text).
pub fn parse_document(doc: &[u8]) -> Result<Report, Reject> {
    if doc.len() < 38 || &doc[..4] != b"TDOC" || doc[4] != 0 || doc[37] == 0 || doc[37] > 48 || doc.len() < 38 + doc[37] as usize {
        return Err(reject("not-a-document", 0));
    }
    if doc[5..37] != [0; 32] || &doc[38..38 + doc[37] as usize] != FORMAT_NAME.as_bytes() {
        return Err(reject("other-format", 0));
    }
    if doc.len() > MAX_DOCUMENT {
        return Err(reject("too-big", 0));
    }
    parse_text(&doc[HEAD..])
}

/// Parses a report given as a file: a whole document, or the text alone.
pub fn parse_any(bytes: &[u8]) -> Result<Report, Reject> {
    if bytes.starts_with(b"TDOC") {
        parse_document(bytes)
    } else {
        if bytes.len() + HEAD > MAX_DOCUMENT {
            return Err(reject("too-big", 0));
        }
        parse_text(bytes)
    }
}

fn int(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 10 || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0')) {
        return None;
    }
    s.parse().ok()
}

/// A decimal with exactly `decimals` digits after the point (and at most
/// 7 before it), optionally negative.
fn fixed(s: &str, decimals: usize, negative: bool) -> Option<f64> {
    let body = match s.strip_prefix('-') {
        Some(b) if negative => b,
        Some(_) => return None,
        None => s,
    };
    let (i, f) = body.split_once('.')?;
    if i.is_empty() || i.len() > 7 || !i.bytes().all(|b| b.is_ascii_digit()) || (i.len() > 1 && i.starts_with('0')) {
        return None;
    }
    if f.len() != decimals || !f.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// A benchmark value: an integer, or a decimal with 1 to 3 digits after
/// the point; at most 7 digits before it.
fn bench_value(s: &str) -> Option<f64> {
    let (i, f) = s.split_once('.').unwrap_or((s, ""));
    if i.is_empty() || i.len() > 7 || !i.bytes().all(|b| b.is_ascii_digit()) || (i.len() > 1 && i.starts_with('0')) {
        return None;
    }
    if s.contains('.') && (f.is_empty() || f.len() > 3 || !f.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    s.parse().ok()
}

fn date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let (Some(y), Some(m), Some(d)) = (int4(&s[..4]), int4(&s[5..7]), int4(&s[8..])) else { return false };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    (2000..=9999).contains(&y) && (1..=12).contains(&m) && d >= 1 && d <= days[m as usize - 1]
}

fn int4(s: &str) -> Option<u32> {
    s.bytes().all(|b| b.is_ascii_digit()).then(|| s.parse().ok()).flatten()
}

/// A string field: printable ASCII, `%` only as `%` and two uppercase hex
/// digits; its unescaped length at most `max` bytes.
fn string(s: &str, max: usize) -> Option<String> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = |c: u8| c.is_ascii_digit() || (b'A'..=b'F').contains(&c);
            if i + 2 >= b.len() || !hex(b[i + 1]) || !hex(b[i + 2]) {
                return None;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    let u = unescape(s);
    (u.len() <= max).then_some(u)
}

/// A version or build field: 1 to 32 printable bytes without `%`, or `-`.
fn version_field(s: &str) -> Option<String> {
    if s == "-" {
        return Some(String::new());
    }
    (s.len() <= 32 && !s.contains('%')).then(|| s.to_string())
}

fn identity(f: &[&str]) -> Option<Identity> {
    let name = string(f[0], MAX_NAME)?;
    if name.is_empty() {
        return None;
    }
    Some(Identity { name, version: version_field(f[1])?, build: version_field(f[2])? })
}

/// The fields each known type needs (after the type word).
fn fields_of(t: &str) -> usize {
    match t {
        "terse-report" | "end" => 1,
        "period" | "warn" | "halt" | "shed" | "program" => 2,
        "host" | "mem" | "check" => 3,
        "runs" | "perf" => 4,
        "late" | "start" => 5,
        "bench" => 6,
        "device" => 5,
        "env" => 7,
        _ => 0,
    }
}

/// Parses a report's text: every rule of `REPORT.md` section 2.
pub fn parse_text(text: &[u8]) -> Result<Report, Reject> {
    if text.is_empty() {
        return Err(reject("first-line", 1));
    }
    if text.last() != Some(&b'\n') {
        return Err(reject("no-final-newline", 0));
    }
    if !text.iter().all(|&b| b == b'\n' || (0x20..0x7F).contains(&b)) {
        return Err(reject("not-text", 0));
    }
    let text = std::str::from_utf8(text).map_err(|_| reject("not-text", 0))?;
    let mut r = Report {
        from: String::new(),
        to: String::new(),
        host: Identity::default(),
        env: Environment::default(),
        devices: Vec::new(),
        runs: 0,
        ended: 0,
        frames: 0,
        late_frames: 0,
        start: None,
        late: Vec::new(),
        warn: Vec::new(),
        halt: Vec::new(),
        shed: None,
        mem: Vec::new(),
        check: Vec::new(),
        perf: None,
        bench: Vec::new(),
        programs: Vec::new(),
    };
    let mut rank = 0usize;
    let mut seen = [0usize; ORDER.len()];
    let mut ended = false;
    let lines: Vec<&str> = text[..text.len() - 1].split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let n = i + 1;
        if ended {
            return Err(reject("after-end", n));
        }
        if line.len() > MAX_LINE {
            return Err(reject("long-line", n));
        }
        if line.is_empty() || line.starts_with(' ') || line.ends_with(' ') || line.contains("  ") {
            return Err(reject("bad-spacing", n));
        }
        let f: Vec<&str> = line.split(' ').collect();
        let t = f[0];
        let Some(k) = ORDER.iter().position(|o| *o == t) else {
            if n == 1 {
                return Err(reject("first-line", 1));
            }
            continue; // a type a later version adds
        };
        if n == 1 && t != "terse-report" {
            return Err(reject("first-line", 1));
        }
        if k < rank {
            return Err(reject("order", n));
        }
        rank = k;
        seen[k] += 1;
        let f = &f[1..];
        if f.len() < fields_of(t) {
            return Err(reject("fields", n));
        }
        let once = matches!(t, "terse-report" | "period" | "host" | "env" | "runs" | "start" | "shed" | "perf" | "end");
        if once && seen[k] > 1 {
            return Err(reject("duplicate", n));
        }
        let bad = || reject("bad-field", n);
        let comp = |s: &str, r: &Report| -> Result<Component, Reject> {
            let c = Component::parse(s).ok_or(reject("bad-field", n))?;
            if host_component(c) || r.devices.iter().any(|d| d.0 == c) {
                Ok(c)
            } else {
                Err(reject("undeclared-component", n))
            }
        };
        let dup = |list: Vec<Component>, c: Component| -> Result<(), Reject> {
            if list.contains(&c) { Err(reject("duplicate", n)) } else { Ok(()) }
        };
        let cap = |len: usize, max: usize| -> Result<(), Reject> { if len >= max { Err(reject("too-many", n)) } else { Ok(()) } };
        match t {
            "terse-report" => {
                if int(f[0]) != Some(FORMAT) {
                    return Err(reject("version", n));
                }
            }
            "period" => {
                if !date(f[0]) || !date(f[1]) || f[0] > f[1] {
                    return Err(reject("bad-period", n));
                }
                r.from = f[0].into();
                r.to = f[1].into();
            }
            "host" => r.host = identity(f).ok_or_else(bad)?,
            "env" => {
                let cores = int(f[3]).filter(|&c| (1..=4096).contains(&c)).ok_or_else(bad)?;
                let ram = int(f[4]).filter(|&g| g <= 65536).ok_or_else(bad)?;
                r.env = Environment {
                    os: string(f[0], 64).ok_or_else(bad)?,
                    os_version: string(f[1], 64).ok_or_else(bad)?,
                    arch: string(f[2], 32).ok_or_else(bad)?,
                    cores,
                    ram_gib: ram,
                    renderer: string(f[5], 32).ok_or_else(bad)?,
                    cpu: string(f[6], 128).ok_or_else(bad)?,
                };
            }
            "device" => {
                cap(r.devices.len(), MAX_DEVICES)?;
                let c = Component::parse(f[0]).filter(|c| c.is_device() || *c == Component::Core).ok_or_else(bad)?;
                dup(r.devices.iter().map(|d| d.0).collect(), c)?;
                let slots: Vec<u8> = if f[1] == "-" {
                    Vec::new()
                } else {
                    f[1].split(',').map(|s| int(s).filter(|&v| (1..=255).contains(&v)).map(|v| v as u8)).collect::<Option<_>>().ok_or_else(bad)?
                };
                r.devices.push((c, slots, identity(&f[2..5]).ok_or_else(bad)?));
            }
            "runs" => {
                let v: Vec<u32> = f[..4].iter().map(|s| int(s)).collect::<Option<_>>().ok_or_else(bad)?;
                if v[1] > v[0] || v[3] > v[2] {
                    return Err(bad());
                }
                (r.runs, r.ended, r.frames, r.late_frames) = (v[0], v[1], v[2], v[3]);
            }
            "start" => {
                let count = int(f[0]).ok_or_else(bad)?;
                let median_ms = fixed(f[1], 1, false).ok_or_else(bad)?;
                let worst_ms = fixed(f[2], 1, false).ok_or_else(bad)?;
                let cold = int(f[3]).filter(|&c| c <= count).ok_or_else(bad)?;
                let cold_worst_ms = if f[4] == "-" { None } else { Some(fixed(f[4], 1, false).ok_or_else(bad)?) };
                if count == 0 || median_ms > worst_ms || (cold == 0) != cold_worst_ms.is_none() {
                    return Err(bad());
                }
                r.start = Some(Start { count, median_ms, worst_ms, cold, cold_worst_ms });
            }
            "late" => {
                cap(r.late.len(), MAX_COMPONENT_LINES)?;
                let c = comp(f[0], &r)?;
                dup(r.late.iter().map(|l| l.0).collect(), c)?;
                let v: Vec<u32> = f[1..5].iter().map(|s| int(s)).collect::<Option<_>>().ok_or_else(bad)?;
                r.late.push((c, [v[0], v[1], v[2], v[3]]));
            }
            "warn" | "halt" => {
                let list = if t == "warn" { &r.warn } else { &r.halt };
                cap(list.len(), MAX_COMPONENT_LINES)?;
                let c = comp(f[0], &r)?;
                dup(list.iter().map(|l| l.0).collect(), c)?;
                let v = int(f[1]).filter(|&v| v >= 1).ok_or_else(bad)?;
                if t == "warn" { r.warn.push((c, v)) } else { r.halt.push((c, v)) }
            }
            "shed" => r.shed = Some((int(f[0]).ok_or_else(bad)?, int(f[1]).ok_or_else(bad)?)),
            "mem" => {
                cap(r.mem.len(), MAX_COMPONENT_LINES)?;
                let c = comp(f[0], &r)?;
                dup(r.mem.iter().map(|m| m.component).collect(), c)?;
                let peak_mib = fixed(f[1], 1, false).ok_or_else(bad)?;
                let growth = if f[2] == "-" { None } else { Some(fixed(f[2], 2, true).ok_or_else(bad)?) };
                r.mem.push(Mem { component: c, peak_mib, growth });
            }
            "check" => {
                cap(r.check.len(), MAX_COMPONENT_LINES)?;
                let c = comp(f[0], &r)?;
                dup(r.check.iter().map(|m| m.0).collect(), c)?;
                let v = fixed(f[1], 1, false).ok_or_else(bad)?;
                let res = f[2];
                if !matches!(res, "ok" | "shed" | "slow" | "absent") {
                    return Err(bad());
                }
                r.check.push((c, v, res.into()));
            }
            "perf" => {
                let id = identity(&f[..3]).ok_or_else(bad)?;
                if !date(f[3]) {
                    return Err(bad());
                }
                r.perf = Some((id, f[3].into()));
            }
            "bench" => {
                if r.perf.is_none() {
                    return Err(reject("bench-without-perf", n));
                }
                cap(r.bench.len(), MAX_BENCH)?;
                let id = f[0];
                if id.len() > MAX_BENCH_ID || id.contains('%') || r.bench.iter().any(|b| b.id == id) {
                    return Err(bad());
                }
                let value = if f[1] == "-" { None } else { Some(bench_value(f[1]).ok_or_else(bad)?) };
                let unit = string(f[2], 16).ok_or_else(bad)?;
                r.bench.push(Bench {
                    id: id.into(),
                    value,
                    unit,
                    requirement: Tier::parse(f[3], "pass", "fail").ok_or_else(bad)?,
                    ceiling: Tier::parse(f[4], "pass", "fail").ok_or_else(bad)?,
                    target: Tier::parse(f[5], "met", "missed").ok_or_else(bad)?,
                });
            }
            "program" => {
                cap(r.programs.len(), MAX_PROGRAMS)?;
                let img = f[0];
                if img.len() != 16 || !img.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
                    return Err(bad());
                }
                r.programs.push((img.into(), int(f[1]).ok_or_else(bad)?));
            }
            "end" => {
                if int(f[0]) != Some(i as u32) {
                    return Err(reject("end-count", n));
                }
                ended = true;
            }
            _ => unreachable!(),
        }
    }
    for (k, t) in ORDER.iter().enumerate() {
        if matches!(*t, "terse-report" | "period" | "host" | "env" | "runs" | "end") && seen[k] == 0 {
            return Err(reject(
                match *t {
                    "period" => "missing-period",
                    "host" => "missing-host",
                    "env" => "missing-env",
                    "runs" => "missing-runs",
                    _ => "missing-end",
                },
                0,
            ));
        }
    }
    Ok(r)
}

// --- Making a report from the record -----------------------------------

/// What the report is made of besides the record.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The period: its last day and its length in days.
    pub to: String,
    pub days: u32,
    /// How many of the programs most often working in blamed frames to
    /// name (opt-in; 0 names none).
    pub programs: usize,
    pub perf: Option<(Identity, String)>,
    pub bench: Vec<Bench>,
}

/// The date `days` before `date` (both `YYYY-MM-DD`).
pub fn days_before(date: &str, days: u32) -> Option<String> {
    if !self::date(date) {
        return None;
    }
    let (y, m, d) = (date[..4].parse::<i64>().ok()?, date[5..7].parse::<i64>().ok()?, date[8..].parse::<i64>().ok()?);
    let n = days_from_civil(y, m, d) - days as i64;
    let (y, m, d) = civil_from_days(n);
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn sat(v: u64) -> u32 {
    v.min(u32::MAX as u64) as u32
}

/// Makes a report from one host identity's record lines (oldest first:
/// a rotated `.1` file's lines before the current file's), keeping the
/// runs begun from `days` days before `to` up to `to`. `None` if no run
/// of the period has a `host` and an `env` line.
pub fn from_record(lines: &[Line], o: &Options) -> Option<Report> {
    let days = o.days.max(1);
    let from = days_before(&o.to, days - 1)?;
    let mut host = None;
    let mut env = None;
    let mut devices: BTreeMap<Component, (Vec<u8>, Identity)> = BTreeMap::new();
    let (mut runs, mut ended, mut frames, mut late_frames) = (0u64, 0u64, 0u64, 0u64);
    let mut starts: Vec<(f64, bool)> = Vec::new();
    let mut late: BTreeMap<Component, [u64; 4]> = BTreeMap::new();
    let mut warn: BTreeMap<Component, u64> = BTreeMap::new();
    let mut halt: BTreeMap<Component, u64> = BTreeMap::new();
    let (mut shed_late, mut shed_check, mut shed_any) = (0u64, 0u64, false);
    // component → (peak KiB, best growth); per run: component → (first s, first KiB, last s, last KiB)
    let mut mem: BTreeMap<Component, (u64, Option<f64>)> = BTreeMap::new();
    let mut run_mem: BTreeMap<Component, (u64, u64, u64, u64)> = BTreeMap::new();
    let mut check: BTreeMap<Component, (f64, String)> = BTreeMap::new();
    let mut by_image: BTreeMap<String, u64> = BTreeMap::new();
    let mut first_date: Option<String> = None;
    let mut inside = false;
    let close_run = |run_mem: &mut BTreeMap<Component, (u64, u64, u64, u64)>, mem: &mut BTreeMap<Component, (u64, Option<f64>)>| {
        for (c, (s0, k0, s1, k1)) in std::mem::take(run_mem) {
            if s1 >= s0 + 60 {
                let g = (k1 as f64 - k0 as f64) / 1024.0 / ((s1 - s0) as f64 / 60.0);
                let e = mem.entry(c).or_insert((0, None));
                e.1 = Some(e.1.map_or(g, |b: f64| b.max(g)));
            }
        }
    };
    for l in lines {
        match l {
            Line::Run { date, .. } => {
                close_run(&mut run_mem, &mut mem);
                inside = date.as_str() >= from.as_str() && date.as_str() <= o.to.as_str();
                if inside {
                    runs += 1;
                    if first_date.as_ref().is_none_or(|d| date < d) {
                        first_date = Some(date.clone());
                    }
                    devices.clear();
                }
            }
            _ if !inside => {}
            Line::Host(i) => host = Some(i.clone()),
            Line::Env(e) => env = Some(e.clone()),
            Line::Device { component, slots, identity } => {
                devices.insert(*component, (slots.clone(), identity.clone()));
            }
            Line::Start { ms, cold } => starts.push((*ms, cold == "build" || cold == "boot")),
            Line::Late { verdict, component, image, .. } => {
                let i = match verdict {
                    Verdict::Blamed => 0,
                    Verdict::Starved => 1,
                    Verdict::Memory => 2,
                    Verdict::Spread => 3,
                };
                late.entry(*component).or_default()[i] += 1;
                if *verdict == Verdict::Blamed && !image.is_empty() {
                    *by_image.entry(image.clone()).or_default() += 1;
                }
            }
            Line::Warn { component, .. } => *warn.entry(*component).or_default() += 1,
            Line::Stop { component, .. } => *halt.entry(*component).or_default() += 1,
            Line::Shed { why, .. } => {
                shed_any = true;
                match why.as_str() {
                    "late" => shed_late += 1,
                    "check" => shed_check += 1,
                    _ => {}
                }
            }
            Line::Mem { seconds, component, rss_kib } => {
                let e = mem.entry(*component).or_insert((0, None));
                e.0 = e.0.max(*rss_kib);
                let r = run_mem.entry(*component).or_insert((*seconds, *rss_kib, *seconds, *rss_kib));
                r.2 = *seconds;
                r.3 = *rss_kib;
            }
            Line::Check { component, ms, result } => {
                check.insert(*component, (*ms, result.clone()));
            }
            Line::End { frames: f, late: n, .. } => {
                ended += 1;
                frames += f;
                late_frames += n;
            }
            _ => {}
        }
    }
    close_run(&mut run_mem, &mut mem);
    let (host, env) = (host?, env?);
    let declared_devices: Vec<Component> = devices.keys().copied().collect();
    let declared = |c: &Component| host_component(*c) || declared_devices.contains(c);
    let start = (!starts.is_empty()).then(|| {
        let mut v: Vec<f64> = starts.iter().map(|s| s.0).collect();
        v.sort_by(f64::total_cmp);
        let cold: Vec<f64> = starts.iter().filter(|s| s.1).map(|s| s.0).collect();
        Start {
            count: sat(v.len() as u64),
            median_ms: v[v.len() / 2],
            worst_ms: v[v.len() - 1],
            cold: sat(cold.len() as u64),
            cold_worst_ms: cold.iter().copied().reduce(f64::max),
        }
    });
    let mut programs: Vec<(String, u32)> = by_image.into_iter().map(|(i, n)| (i, sat(n))).collect();
    programs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    programs.truncate(o.programs.min(MAX_PROGRAMS));
    let mut devices: Vec<(Component, Vec<u8>, Identity)> = devices.into_iter().map(|(c, (s, i))| (c, s, i)).collect();
    devices.truncate(MAX_DEVICES);
    let mut r = Report {
        // The period begins on the first day the record has a run in it.
        from: first_date.unwrap_or(from),
        to: o.to.clone(),
        host,
        env,
        runs: sat(runs),
        ended: sat(ended).min(sat(runs)),
        frames: sat(frames),
        late_frames: sat(late_frames).min(sat(frames)),
        start,
        late: late.into_iter().filter(|(c, _)| declared(c)).map(|(c, v)| (c, v.map(sat))).take(MAX_COMPONENT_LINES).collect(),
        warn: warn.into_iter().filter(|(c, _)| declared(c)).map(|(c, n)| (c, sat(n))).take(MAX_COMPONENT_LINES).collect(),
        halt: halt.into_iter().filter(|(c, _)| declared(c)).map(|(c, n)| (c, sat(n))).take(MAX_COMPONENT_LINES).collect(),
        shed: shed_any.then_some((sat(shed_late), sat(shed_check))),
        mem: mem
            .into_iter()
            .filter(|(c, _)| declared(c))
            .map(|(c, (k, g))| Mem { component: c, peak_mib: (k as f64 / 1024.0 * 10.0).round() / 10.0, growth: g.map(|g| (g * 100.0).round() / 100.0) })
            .take(MAX_COMPONENT_LINES)
            .collect(),
        check: check.into_iter().filter(|(c, _)| declared(c)).map(|(c, (v, r))| (c, v, r)).take(MAX_COMPONENT_LINES).collect(),
        perf: o.perf.clone(),
        bench: o.bench.iter().take(MAX_BENCH).cloned().collect(),
        programs,
        devices,
    };
    // `-0.00` is written `0.00`.
    for m in &mut r.mem {
        if let Some(g) = m.growth
            && g == 0.0
        {
            m.growth = Some(0.0);
        }
    }
    Some(r)
}

/// A plain account of a report, for `terse perf report --check`.
pub fn describe(r: &Report) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "host {} ({} to {})", r.host.line(), r.from, r.to);
    let _ = writeln!(o, "computer: {} {} {}, {} cores, {} GiB, renderer {}, {}", r.env.os, r.env.os_version, r.env.arch, r.env.cores, r.env.ram_gib, r.env.renderer, r.env.cpu);
    for (c, _, i) in &r.devices {
        let _ = writeln!(o, "device {c}: {}", i.line());
    }
    let _ = writeln!(o, "{} runs ({} ended cleanly): {} frames, {} late", r.runs, r.ended, r.frames, r.late_frames);
    o
}
