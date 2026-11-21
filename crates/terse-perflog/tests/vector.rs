//! `vectors/perf-record.txt` (from `example-record.txt`), the example
//! record of `RECORD.md`: every line parses, writes back as it was, and
//! sums up as `terse perf log` says.

use terse_perflog::record::{self, Line};
use terse_perflog::summary::Summary;
use terse_perflog::{Component, Verdict};

const VECTOR: &str = include_str!("../example-record.txt");

#[test]
fn the_example_record() {
    let lines = record::parse(VECTOR);
    assert_eq!(lines.len(), VECTOR.lines().count(), "every line parses");
    for (text, line) in VECTOR.lines().zip(&lines) {
        assert_eq!(line.encode(), text, "{line:?}");
    }
    assert_eq!(lines[0], Line::Format(1));
    assert!(matches!(&lines[3], Line::Env(e) if e.cpu == "Intel(R) Core(TM) i7-10750H CPU @ 2.60GHz" && e.ram_gib == 15));
    assert!(matches!(&lines[4], Line::Device { component: Component::Device(2), slots, identity } if slots == &[2, 10] && identity.name == "terse-dev-terminal (Rust)"));
    assert!(matches!(&lines[9], Line::Readings { component: Component::Core, readings } if readings.cpu && readings.wait && readings.stall));
    assert!(matches!(&lines[10], Line::Readings { component: Component::Device(2), readings } if readings.cpu && readings.wait && !readings.stall));
    assert!(matches!(&lines[16], Line::Late { frame: 1200, verdict: Verdict::Blamed, component: Component::Device(7), blocked_ms, .. } if *blocked_ms == 46.5));
    assert!(matches!(lines.iter().find(|l| matches!(l, Line::Other(_))), Some(Line::Other(s)) if s == "future-type 1 2 3"));

    let mut s = Summary::default();
    s.add(&lines);
    let r = s.report();
    for want in [
        "terse-bus 0.1.0 b3:5faccb21bc9113fb",
        "start-up to the first frame: 1 runs, median 131 ms",
        "frames: 3600 in runs that ended cleanly, late 6 (6 recorded)",
        "dev7: blamed 3, starved 1",
        "abababababababab: 3",
        "frame 3: 815.2 ms, spread outside",
        "dev7: 20.0 to 80.0 MiB over 600 s (+6.00 MiB a minute",
        "warning: frame 1203: dev7 late 3 times",
        "stopped: frame 1450: dev7, machine 2 (program abababababababab), saved slot17",
        "hardware check: dev3 6.5 ms: ok",
        "lines lost (the writer fell behind): 2",
    ] {
        assert!(r.contains(want), "{want}\n{r}");
    }
}
