//! `terse perf report` and `terse perf aggregate` (RECORD.md 5): a
//! performance report made from this computer's record, printed in full,
//! published only when the user says so; and the verdicts of many
//! reports, the reference for programs/reports.

use std::collections::BTreeSet;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use terse_perflog::aggregate::{self, Aggregate, Submission};
use terse_perflog::record::{self, Line};
use terse_perflog::report::{self, Bench, MAX_BENCH_ID, Options, Report, Tier};
use terse_perflog::Identity;

use crate::{Args, relay_urls};

pub const USAGE: &str = "usage:
  terse perf report [--days <n>] [--to <date>] [--host <name>] [--data <dir> | --record <dir or file>]
                    [--run [--runs <n>]] [--programs <n>] [--out <file>]
                    [--publish [--yes] [--relay ws://...|none] [--store <dir>] [--config <dir>]]
  terse perf report --check <file>
  terse perf aggregate [--follow <keys file>] <report file or dir>...";

/// A row of a `terse perf` run, as `perf.rs` has it.
pub struct PerfRow {
    pub id: String,
    pub what: String,
    pub measured: String,
    pub rule: String,
    /// The requirement's outcome, if the row has one.
    pub requirement: Option<bool>,
}

/// The first number in a text and the unit after it (`frame` as
/// `frames`, a relay message as `trips`).
fn first_number(s: &str) -> Option<(f64, String)> {
    let words: Vec<&str> = s.split(|c: char| c.is_whitespace() || matches!(c, ';' | ',' | '(' | ')')).filter(|w| !w.is_empty()).collect();
    for (i, w) in words.iter().enumerate() {
        if let Ok(v) = w.parse::<f64>()
            && v.is_finite()
            && v >= 0.0
        {
            let unit = words.get(i + 1).copied().unwrap_or("");
            if unit == "thread" || unit == "threads" {
                continue; // "1 thread p50 6.5 ms": the threads are not the measure
            }
            let unit = match unit {
                "frame" => "frames",
                "Get" | "Subscribe" => "trips",
                u if !u.is_empty() && u.len() <= 16 && u.chars().all(|c| c.is_ascii_alphabetic()) => u,
                _ => "",
            };
            return Some((v, unit.to_string()));
        }
    }
    None
}

/// A `terse perf` run's rows as `bench` lines: the id (a `W` row's with
/// its scenario; made unique), the first number measured and its unit,
/// the requirement, and whether a target `T: ≤ <n> <unit>` was met.
/// (Ceilings are not yet separate rows of `terse perf`: `-`.)
pub fn bench_of(rows: &[PerfRow]) -> Vec<Bench> {
    let mut out: Vec<Bench> = Vec::new();
    for r in rows {
        let mut id = if r.id == "W" { format!("W.{}", r.what.split(':').next().unwrap_or("").trim()) } else { r.id.clone() };
        id.retain(|c| c.is_ascii_graphic() && c != '%');
        id.truncate(MAX_BENCH_ID - 3);
        if id.is_empty() {
            continue;
        }
        let base = id.clone();
        let mut n = 2;
        while out.iter().any(|b| b.id == id) {
            id = format!("{base}.{n}");
            n += 1;
        }
        let measured = first_number(&r.measured).map(|(v, u)| ((v * 1000.0).round() / 1000.0, u)).filter(|(v, _)| *v < 1e7);
        let target = match (r.rule.split_once("T: ≤ ").and_then(|(_, t)| first_number(t)), &measured) {
            (Some((limit, unit)), Some((v, u))) if unit == *u => {
                if *v <= limit {
                    Tier::Pass
                } else {
                    Tier::Fail
                }
            }
            _ => Tier::None,
        };
        out.push(Bench {
            id,
            value: measured.as_ref().map(|m| m.0),
            unit: measured.map(|m| m.1).unwrap_or_default(),
            requirement: match r.requirement {
                Some(true) => Tier::Pass,
                Some(false) => Tier::Fail,
                None => Tier::None,
            },
            ceiling: Tier::None,
            target,
        });
    }
    out
}

/// The record files of a directory, by host identity (file stem), each
/// group's rotated file first.
fn record_groups(path: &Path) -> Result<Vec<(String, Vec<Line>)>, String> {
    let mut groups: std::collections::BTreeMap<String, Vec<PathBuf>> = Default::default();
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
    let mut out = Vec::new();
    for (stem, mut files) in groups {
        files.sort_by_key(|p| !p.to_string_lossy().ends_with(".1"));
        let mut lines = Vec::new();
        for f in files {
            let text = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
            lines.extend(record::parse(&text));
        }
        out.push((stem, lines));
    }
    Ok(out)
}

/// The confirmation `--publish` shows under the report.
pub fn confirmation(r: &Report, key: &str) -> String {
    format!(
        "This report is about to be published as a document of your feed,\n\
         titled \"{}\",\n\
         signed with your feed key {key}.\n\
         \n\
         - It is PUBLIC: anyone who follows your feed or finds it on a relay can\n\
         \x20 read every line above: which host and devices you run, how they\n\
         \x20 performed, and the kind of computer (no name, address or path).\n\
         - It CANNOT BE DELETED: once a relay holds it, there is no way to take it\n\
         \x20 back. Tersenet has no retraction.\n\
         - It is voluntary: nothing is published unless you say yes here, and\n\
         \x20 nothing is ever sent automatically. To stop, do not publish again.\n\
         - Aggregators count your newest report per host build; one report per\n\
         \x20 period is enough.\n\
         \n\
         Publish it? Type yes to publish, anything else cancels: ",
        r.title()
    )
}

fn today() -> String {
    record::utc_date()
}

/// `terse perf report`.
pub fn report(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &["--run", "--publish", "--yes"])?;
    args.check_known(&[
        "--days", "--to", "--host", "--data", "--record", "--run", "--runs", "--programs", "--out", "--publish", "--yes", "--check", "--relay", "--store",
        "--config", "--no-feed-relays",
    ])
    .map_err(|_| USAGE.to_string())?;
    if !args.positional.is_empty() {
        return Err(USAGE.into());
    }
    if let Some(file) = args.get("--check") {
        return check(file);
    }
    let days: u32 = args.get("--days").map_or(Ok(30), |d| d.parse().ok().filter(|&d| (1..=366).contains(&d)).ok_or("--days: 1 to 366"))?;
    let to = args.get("--to").map_or_else(today, str::to_string);
    if report::days_before(&to, 0).is_none() {
        return Err(format!("--to {to}: not a date (YYYY-MM-DD)"));
    }
    let programs: usize = args.get("--programs").map_or(Ok(0), |n| n.parse().ok().filter(|&n| n <= report::MAX_PROGRAMS).ok_or("--programs: 0 to 5"))?;
    let path = match (args.get("--record"), args.get("--data")) {
        (Some(r), _) => PathBuf::from(r),
        (None, Some(d)) => PathBuf::from(d).join("perf"),
        (None, None) => record::default_dir().ok_or("no data directory (use --data or --record)")?,
    };
    if !path.exists() {
        return Err(format!("no record in {} (a host's runs in a window write it, RECORD.md)", path.display()));
    }
    let mut perf = None;
    let mut bench = Vec::new();
    if args.has("--run") {
        let runs: usize = args.get("--runs").map_or(Ok(3), |n| n.parse().ok().filter(|&n| n > 0).ok_or("bad --runs"))?;
        eprintln!("terse perf report: running terse perf ({runs} runs); this takes a minute or two");
        bench = crate::perf::bench_for_report(runs)?;
        perf = Some((Identity::this("terse"), today()));
    }
    let o = Options { to: to.clone(), days, programs, perf, bench };
    let mut made: Vec<(String, u32, Report)> = Vec::new();
    for (stem, lines) in record_groups(&path)? {
        let Some(r) = report::from_record(&lines, &o) else { continue };
        if let Some(h) = args.get("--host")
            && !stem.contains(h)
            && !r.host.name.contains(h)
        {
            continue;
        }
        let last = lines
            .iter()
            .filter_map(|l| match l {
                Line::Run { date, .. } if date.as_str() <= to.as_str() => Some(date.clone()),
                _ => None,
            })
            .max()
            .unwrap_or_default();
        made.push((last, r.runs, r));
    }
    made.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let Some((_, _, r)) = made.first() else {
        return Err(format!("no runs in the {days} days to {to} in {}: nothing to report", path.display()));
    };
    for (_, _, other) in &made[1..] {
        eprintln!("terse perf report: also recorded: {} (choose with --host)", other.host.line());
    }
    let doc = r.document();
    print!("{}", r.text());
    eprintln!(
        "({} bytes as a document{}; program IDs {})",
        doc.len(),
        if doc.len() > report::TARGET { ", over the 4 KiB target" } else { "" },
        if r.programs.is_empty() { "left out" } else { "included as asked (--programs)" }
    );
    if let Some(out) = args.get("--out") {
        std::fs::write(out, &doc).map_err(|e| format!("{out}: {e}"))?;
        eprintln!("terse perf report: the document written to {out}");
    }
    if args.has("--publish") {
        return publish(&args, r, &doc);
    }
    Ok(ExitCode::SUCCESS)
}

fn publish(args: &Args, r: &Report, doc: &[u8]) -> Result<ExitCode, String> {
    let identity = crate::feed::load_identity(args)?;
    let key = terse_net::to_hex(&identity.feed_key());
    print!("\n{}", confirmation(r, &format!("{}…{}", &key[..6], &key[58..])));
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    if args.has("--yes") {
        println!("yes (--yes)");
    } else {
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer).map_err(|e| e.to_string())?;
        if answer.trim() != "yes" {
            println!("Not published.");
            return Ok(ExitCode::SUCCESS);
        }
    }
    let relays = relay_urls(args, true)?;
    let mut node = crate::feed::open_node(args, &relays)?;
    node.set_identity(identity);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs().min(u32::MAX as u64) as u32);
    match node.publish_document(doc, &r.title(), now) {
        terse_net::Done::Published { seq, blob } => {
            if relays.is_empty() {
                eprintln!("terse: no relay: stored locally only");
            }
            println!("Published: seq {seq} blob {}", terse_net::to_hex(&blob));
            Ok(ExitCode::SUCCESS)
        }
        terse_net::Done::Refused => Err("the relays refused the entry".into()),
        _ => Err("publish failed (store error, or no relay accepted it)".into()),
    }
}

/// `--check <file>`: whether a file is a valid report, and its text.
fn check(file: &str) -> Result<ExitCode, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
    match report::parse_any(&bytes) {
        Ok(r) => {
            print!("{}", r.text());
            println!("valid: a performance report, format {}, {} bytes as a document", report::FORMAT, r.document().len());
            print!("{}", report::describe(&r));
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            println!("not a valid performance report: {e}");
            Ok(ExitCode::from(1))
        }
    }
}

/// The reporter and entry a file stands for: `<feed key hex>-<seq>…` in
/// its name, else the file's own name and its place among the arguments.
fn key_seq(path: &Path, index: usize) -> (String, u64) {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let b = name.as_bytes();
    if b.len() > 65 && b[..64].iter().all(|c| c.is_ascii_hexdigit()) && b[64] == b'-' {
        let seq: String = name[65..].chars().take_while(char::is_ascii_digit).collect();
        if let Ok(seq) = seq.parse() {
            return (name[..64].to_ascii_lowercase(), seq);
        }
    }
    (path.display().to_string(), index as u64)
}

/// `terse perf aggregate`.
pub fn aggregate(args: &[String]) -> Result<ExitCode, String> {
    let args = Args::parse(args, &[])?;
    args.check_known(&["--follow"]).map_err(|_| USAGE.to_string())?;
    if args.positional.is_empty() {
        return Err(USAGE.into());
    }
    let follow: Option<BTreeSet<String>> = match args.get("--follow") {
        Some(f) => Some(
            std::fs::read_to_string(f)
                .map_err(|e| format!("{f}: {e}"))?
                .lines()
                .map(|l| l.trim().to_ascii_lowercase())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect(),
        ),
        None => None,
    };
    let mut files = Vec::new();
    for p in &args.positional {
        let p = PathBuf::from(p);
        if p.is_dir() {
            let mut inside: Vec<PathBuf> = std::fs::read_dir(&p).map_err(|e| format!("{}: {e}", p.display()))?.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
            inside.sort();
            files.extend(inside);
        } else {
            files.push(p);
        }
    }
    let mut parsed = Vec::new();
    let mut malformed = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let bytes = std::fs::read(f).map_err(|e| format!("{}: {e}", f.display()))?;
        match report::parse_any(&bytes) {
            Ok(r) => parsed.push((key_seq(f, i), r)),
            Err(e) => malformed.push(format!("{}: {e}", f.display())),
        }
    }
    let mut a = Aggregate::default();
    for ((key, seq), r) in &parsed {
        a.add(&Submission { key: key.clone(), seq: *seq, report: r });
    }
    for m in &malformed {
        println!("malformed: {m}");
    }
    let all = |_: &str| true;
    let text = match &follow {
        Some(set) => {
            let followed = |k: &str| set.contains(k);
            aggregate::render(&a, malformed.len(), &[("feeds you follow", &followed), ("all found", &all)])
        }
        None => aggregate::render(&a, malformed.len(), &[("all given", &all)]),
    };
    print!("{text}");
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, what: &str, measured: &str, rule: &str, requirement: Option<bool>) -> PerfRow {
        PerfRow { id: id.into(), what: what.into(), measured: measured.into(), rule: rule.into(), requirement }
    }

    #[test]
    fn perf_rows_as_bench_lines() {
        let b = bench_of(&[
            row("M1a", "held get", "1 frame; service 0.1 ms; relay asked 0 times", "R6: ≤ 2 frames; R1: no relay asked", Some(true)),
            row("M2", "get", "1 Get; service 6.2 ms", "R2: 1 round trip", Some(false)),
            row("H1", "start", "22 ms (20–28)", "T: ≤ 500 ms", None),
            row("H6", "cjk", "p50 22 ms, max 26 ms", "T: ≤ 16.7 ms", None),
            row("W", "vector: worst", "1 thread p50 6.5 ms, max 7.0 ms; caps", "R10: within the caps; T: ≤ 16.7 ms", Some(true)),
            row("W", "vector: worst again", "never", "T: ≤ 16.7 ms", None),
        ]);
        let lines: Vec<String> = b
            .iter()
            .map(|b| format!("{} {:?} {} {:?} {:?}", b.id, b.value, b.unit, b.requirement, b.target))
            .collect();
        assert_eq!(
            lines,
            [
                "M1a Some(1.0) frames Pass None",
                "M2 Some(1.0) trips Fail None",
                "H1 Some(22.0) ms None Pass",
                "H6 Some(22.0) ms None Fail",
                "W.vector Some(6.5) ms Pass Pass",
                "W.vector.2 None  None None",
            ]
        );
    }
}
