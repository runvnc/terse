//! `vectors/perf-reports.txt` (from `example-reports.txt`): the valid
//! reports parse (and write back as they were), the malformed ones are
//! rejected for the reason given; a report made from the example record;
//! the aggregation rule (RECORD.md 5.5).

use terse_perflog::aggregate::{Aggregate, Class, Kind, Submission, tally};
use terse_perflog::record;
use terse_perflog::report::{self, Bench, Options, Report, Tier};

const VECTORS: &str = include_str!("../example-reports.txt");
const RECORD: &str = include_str!("../example-record.txt");

/// One `report` record of the vectors file.
pub struct Vector {
    pub name: String,
    pub valid: bool,
    pub reason: String,
    pub document: Vec<u8>,
}

fn unquote(s: &str) -> String {
    let s = s.strip_prefix('"').unwrap().strip_suffix('"').unwrap();
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            out.push(match it.next().unwrap() {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                c => c,
            });
        } else {
            out.push(c);
        }
    }
    out
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

pub fn vectors() -> Vec<Vector> {
    let mut out = Vec::new();
    for rec in VECTORS.split("\n\n").filter(|r| r.starts_with("report ")) {
        let mut v = Vector { name: String::new(), valid: false, reason: String::new(), document: Vec::new() };
        let mut header = report::header();
        let mut text = String::new();
        for line in rec.lines() {
            if let Some(n) = line.strip_prefix("report ") {
                v.name = n.into();
            } else if let Some((k, val)) = line.trim_start().split_once(' ') {
                match k {
                    "valid" => v.valid = val == "yes",
                    "reason" => v.reason = val.into(),
                    "format" => {
                        header = b"TDOC\0".to_vec();
                        header.extend([0; 32]);
                        header.push(val.len() as u8);
                        header.extend(val.as_bytes());
                    }
                    "header-hex" => header = hex(val),
                    "text" => text = unquote(val),
                    _ => {}
                }
            }
        }
        v.document = [header, text.into_bytes()].concat();
        out.push(v);
    }
    out
}

#[test]
fn the_vectors() {
    let all = vectors();
    assert!(all.len() > 20);
    for v in &all {
        let got = report::parse_document(&v.document);
        if v.valid {
            let r = got.unwrap_or_else(|e| panic!("{}: {e}", v.name));
            if v.name != "later-version-lines" {
                assert_eq!(r.document(), v.document, "{} writes back as it was", v.name);
            }
        } else {
            assert_eq!(got.map(|_| ()).map_err(|e| e.code), Err(v.reason.as_str()), "{}", v.name);
        }
    }
    let full = report::parse_document(&all[0].document).unwrap();
    assert_eq!(full.host.name, "terse-bus");
    assert_eq!(full.env.cpu, "Intel(R) Core(TM) i7-10750H CPU @ 2.60GHz");
    assert_eq!(full.devices[0].2.name, "terse-dev-terminal (Rust)");
    assert_eq!(full.bench[3], Bench { id: "H6".into(), value: Some(22.4), unit: "ms".into(), requirement: Tier::None, ceiling: Tier::None, target: Tier::Fail });
    assert!(all[0].document.len() <= report::TARGET, "the full example is within the target");
}

#[test]
fn too_big_and_header_checks() {
    let minimal = &vectors()[1].document;
    let mut big = minimal[..minimal.len() - 6].to_vec();
    let mut n = 5;
    while big.len() <= report::MAX_DOCUMENT {
        big.extend(b"padding 0123456789012345678901234567890123456789\n");
        n += 1;
    }
    big.extend(format!("end {n}\n").as_bytes());
    assert_eq!(report::parse_document(&big).unwrap_err().code, "too-big");
    // The text alone (a file) parses too.
    assert!(report::parse_any(&minimal[report::HEAD..]).is_ok());
}

fn example() -> Report {
    let o = Options {
        to: "2026-11-21".into(),
        days: 30,
        programs: 0,
        perf: None,
        bench: Vec::new(),
    };
    report::from_record(&record::parse(RECORD), &o).unwrap()
}

#[test]
fn a_report_from_the_example_record() {
    let r = example();
    let text = r.text();
    for want in [
        "terse-report 1\n",
        "period 2026-11-21 2026-11-21\n",
        "host terse-bus 0.1.0 b3:5faccb21bc9113fb\n",
        "device dev7 7 terse-dev-page%20(Rust) 0.1.0 b3:f8f5a88bb33ad6eb\n",
        "runs 1 1 3600 6\n",
        "start 1 131.4 131.4 1 131.4\n",
        "late dev7 3 1 0 0\n",
        "warn dev7 1\n",
        "halt dev7 1\n",
        "shed 1 0\n",
        "mem dev7 80.0 6.00\n",
        "check dev3 6.5 ok\n",
    ] {
        assert!(text.contains(want), "{want}{text}");
    }
    assert!(!text.contains("abababab"), "no program named unless asked");
    assert_eq!(report::parse_document(&r.document()).unwrap(), r, "{text}");
    // Opt-in: the programs.
    let o = Options { to: "2026-11-21".into(), days: 30, programs: 5, perf: None, bench: Vec::new() };
    let r = report::from_record(&record::parse(RECORD), &o).unwrap();
    assert!(r.text().contains("program abababababababab 3\n"), "{}", r.text());
    // A period that ends before the run: nothing to report.
    let o = Options { to: "2026-11-20".into(), days: 30, ..o };
    assert!(report::from_record(&record::parse(RECORD), &o).is_none());
    assert_eq!(report::days_before("2026-03-01", 1).unwrap(), "2026-02-28");
    assert_eq!(report::days_before("2024-03-01", 1).unwrap(), "2024-02-29");
}

/// A host report with the given computer, perhaps halting dev3.
fn synthetic(cores: u32, ram: u32, halted: bool, start: f64) -> Report {
    let mut r = report::parse_document(&vectors()[1].document).unwrap();
    r.env.cores = cores;
    r.env.ram_gib = ram;
    r.devices.push((terse_perflog::Component::Device(3), vec![3], terse_perflog::Identity { name: "vec".into(), version: "1".into(), build: "b".into() }));
    r.frames = 10_000;
    r.ended = 1;
    r.start = Some(report::Start { count: 1, median_ms: start, worst_ms: start, cold: 0, cold_worst_ms: None });
    if halted {
        r.late.push((terse_perflog::Component::Device(3), [20, 5, 0, 0]));
        r.halt.push((terse_perflog::Component::Device(3), 1));
    }
    r
}

#[test]
fn the_rule() {
    let mut a = Aggregate::default();
    let reports: Vec<(String, u64, Report)> = vec![
        ("k1".into(), 1, synthetic(8, 16, true, 100.0)),
        ("k1".into(), 2, synthetic(8, 16, false, 100.0)), // newer: replaces
        ("k2".into(), 1, synthetic(4, 8, true, 200.0)),
        ("k3".into(), 1, synthetic(4, 4, false, 300.0)),
        ("k4".into(), 1, synthetic(2, 16, true, 400.0)), // below the floor
        ("k5".into(), 1, synthetic(16, 32, false, 500.0)),
    ];
    for (k, seq, r) in &reports {
        a.add(&Submission { key: k.clone(), seq: *seq, report: r });
    }
    let dev = a.groups.iter().find(|(g, _)| g.0 == Kind::Device).unwrap().1;
    assert_eq!(dev.len(), 5, "one per key");
    let t = tally(dev.values());
    assert_eq!((t.reporters, t.below, t.judged, t.failing), (5, 1, 4, 1));
    assert_eq!(t.verdict(), "TOO FEW REPORTS (4)");
    // A fifth judged reporter that halts: 2 of 5 fail, at least 1 in 3.
    let r6 = synthetic(4, 4, true, 600.0);
    a.add(&Submission { key: "k6".into(), seq: 1, report: &r6 });
    let dev = a.groups.iter().find(|(g, _)| g.0 == Kind::Device).unwrap().1;
    let t = tally(dev.values());
    assert_eq!(t.verdict(), "NON-CONFORMANT IN SAMPLE (2 of 5 fail: halted 2)");
    assert_eq!(t.median, Some(0), "blamed per 10,000 frames: 0 0 0 20 20");
    let host = a.groups.iter().find(|(g, _)| g.0 == Kind::Host).unwrap().1;
    let t = tally(host.values());
    assert_eq!(t.verdict(), "CONFORMANT IN SAMPLE (5 reporters)");
    assert_eq!(t.median, Some(3000), "the median start-up, tenths of a ms");
    // One more that does not fail: 2 of 6 is still 1 in 3; 2 of 7 is not.
    for k in ["k7", "k8"] {
        let r = synthetic(8, 8, false, 100.0);
        a.add(&Submission { key: k.into(), seq: 1, report: &r });
        let dev = a.groups.iter().find(|(g, _)| g.0 == Kind::Device).unwrap().1;
        let v = tally(dev.values()).verdict();
        assert_eq!(v.starts_with("NON-CONFORMANT"), k == "k7", "{k}: {v}");
    }
    assert_eq!(Class::of(3, 64), Class::Small);
    assert_eq!(Class::of(8, 15), Class::Medium);
}
