//! Aggregating published performance reports (`REPORT.md` section 5): one
//! report per feed key and identity, grouped by host, device and `terse
//! perf` identity, a verdict per group in a stated sample. The reference
//! for `programs/reports`, whose screens the tests compare with it.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::identity::Identity;
use crate::judge::Component;
use crate::report::{Report, Tier};

/// The fewest judged reporters (at or above the hardware floor) for a
/// verdict.
pub const MIN_REPORTERS: usize = 5;
/// A group is non-conformant in a sample when at least one in this many
/// judged reporters fail.
pub const ONE_IN: usize = 3;

/// What kind of identity a group is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Host,
    Device,
    Perf,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Host => "host",
            Kind::Device => "device",
            Kind::Perf => "perf",
        }
    }
}

/// A computer's class by its cores and RAM (`env`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Fewer than 4 logical CPUs or less than 4 GiB: below the hardware
    /// floor, listed but not judged.
    Small,
    /// At least 4 and 4 GiB, but fewer than 8 or less than 16 GiB.
    Medium,
    /// At least 8 logical CPUs and 16 GiB.
    Large,
}

impl Class {
    pub fn of(cores: u32, ram_gib: u32) -> Class {
        if cores < 4 || ram_gib < 4 {
            Class::Small
        } else if cores >= 8 && ram_gib >= 16 {
            Class::Large
        } else {
            Class::Medium
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Class::Small => "small",
            Class::Medium => "medium",
            Class::Large => "large",
        }
    }
}

/// What one reporter's newest report says about one group.
#[derive(Clone, Debug, PartialEq)]
pub struct Fact {
    pub seq: u64,
    pub class: Class,
    /// R15 stopped a machine for it.
    pub halted: bool,
    /// A `terse perf` requirement or ceiling failed (perf groups only).
    pub requirement: bool,
    pub ceiling: bool,
    /// A late frame was blamed on it; the host warned about it.
    pub blamed: bool,
    pub warned: bool,
    /// The group's key measurement: a host's median start-up (tenths of a
    /// ms), a device's blamed frames per 10,000 frames, a perf run's
    /// targets missed.
    pub measure: Option<u64>,
}

impl Fact {
    pub fn failing(&self) -> bool {
        self.halted || self.requirement || self.ceiling
    }
}

/// One report as gathered: from whom, which entry, and what it parsed to.
pub struct Submission<'a> {
    /// The reporter: a feed key (or, for files without one, any name).
    pub key: String,
    pub seq: u64,
    pub report: &'a Report,
}

/// The facts of every group: (kind, identity) → reporter key → fact.
#[derive(Default)]
pub struct Aggregate {
    pub groups: BTreeMap<(Kind, String, String, String), BTreeMap<String, Fact>>,
    pub reports: usize,
}

fn is_host(c: Component) -> bool {
    matches!(c, Component::Core | Component::Compose | Component::Present | Component::Outside)
}

/// The groups a report speaks about and its facts for each.
pub fn facts(r: &Report, seq: u64) -> Vec<((Kind, Identity), Fact)> {
    let class = Class::of(r.env.cores, r.env.ram_gib);
    let blank = Fact { seq, class, halted: false, requirement: false, ceiling: false, blamed: false, warned: false, measure: None };
    let blamed = |f: &dyn Fn(Component) -> bool| -> u64 { r.late.iter().filter(|l| f(l.0)).map(|l| l.1[0] as u64).sum() };
    let mut out: Vec<((Kind, Identity), Fact)> = Vec::new();
    let host_blamed = blamed(&is_host);
    out.push((
        (Kind::Host, r.host.clone()),
        Fact {
            halted: r.halt.iter().any(|h| is_host(h.0)),
            blamed: host_blamed > 0,
            warned: r.warn.iter().any(|w| is_host(w.0)),
            measure: r.start.as_ref().map(|s| (s.median_ms * 10.0).round() as u64),
            ..blank.clone()
        },
    ));
    // Devices: one group per identity (a driver serving two components
    // counts once), the host's own identity not again.
    let mut devs: Vec<(Identity, Vec<Component>)> = Vec::new();
    for (c, _, id) in &r.devices {
        if !c.is_device() || *id == r.host {
            continue;
        }
        match devs.iter_mut().find(|d| d.0 == *id) {
            Some(d) => d.1.push(*c),
            None => devs.push((id.clone(), vec![*c])),
        }
    }
    for (id, comps) in devs {
        let mine = |c: Component| comps.contains(&c);
        let b = blamed(&mine);
        out.push((
            (Kind::Device, id),
            Fact {
                halted: r.halt.iter().any(|h| mine(h.0)),
                blamed: b > 0,
                warned: r.warn.iter().any(|w| mine(w.0)),
                measure: (r.frames > 0).then(|| b.min(r.frames as u64) * 10_000 / r.frames as u64),
                ..blank.clone()
            },
        ));
    }
    if let Some((id, _)) = &r.perf {
        out.push((
            (Kind::Perf, id.clone()),
            Fact {
                requirement: r.bench.iter().any(|b| b.requirement == Tier::Fail),
                ceiling: r.bench.iter().any(|b| b.ceiling == Tier::Fail),
                measure: Some(r.bench.iter().filter(|b| b.target == Tier::Fail).count() as u64),
                ..blank
            },
        ));
    }
    out
}

impl Aggregate {
    /// Adds a report: for each of its groups, its fact replaces the
    /// reporter's earlier one if its `seq` is higher (one report per feed
    /// key and identity: the newest).
    pub fn add(&mut self, s: &Submission) {
        self.reports += 1;
        for ((kind, id), fact) in facts(s.report, s.seq) {
            let g = self.groups.entry((kind, id.name, id.version, id.build)).or_default();
            match g.get(&s.key) {
                Some(old) if old.seq >= fact.seq => {}
                _ => {
                    g.insert(s.key.clone(), fact);
                }
            }
        }
    }
}

/// A group's numbers in one sample, and its verdict.
#[derive(Clone, Debug, PartialEq)]
pub struct Tally {
    pub reporters: usize,
    pub below: usize,
    pub judged: usize,
    pub failing: usize,
    pub halted: usize,
    pub requirement: usize,
    pub ceiling: usize,
    pub blamed: usize,
    pub warned: usize,
    pub median: Option<u64>,
}

/// Counts the facts of the reporters `keep` admits.
pub fn tally<'a>(facts: impl Iterator<Item = &'a Fact>) -> Tally {
    let mut t = Tally { reporters: 0, below: 0, judged: 0, failing: 0, halted: 0, requirement: 0, ceiling: 0, blamed: 0, warned: 0, median: None };
    let mut measures = Vec::new();
    for f in facts {
        t.reporters += 1;
        if f.class == Class::Small {
            t.below += 1;
            continue;
        }
        t.judged += 1;
        if f.failing() {
            t.failing += 1;
            t.halted += f.halted as usize;
            t.requirement += f.requirement as usize;
            t.ceiling += f.ceiling as usize;
        }
        t.blamed += f.blamed as usize;
        t.warned += f.warned as usize;
        if let Some(m) = f.measure {
            measures.push(m);
        }
    }
    measures.sort_unstable();
    t.median = measures.get(measures.len() / 2).copied().filter(|_| !measures.is_empty());
    t
}

impl Tally {
    /// The verdict, in the words of `REPORT.md` section 5.
    pub fn verdict(&self) -> String {
        if self.judged < MIN_REPORTERS {
            return format!("TOO FEW REPORTS ({})", self.judged);
        }
        if self.failing * ONE_IN >= self.judged {
            let mut why = Vec::new();
            for (n, w) in [(self.halted, "halted"), (self.requirement, "requirement"), (self.ceiling, "ceiling")] {
                if n > 0 {
                    why.push(format!("{w} {n}"));
                }
            }
            return format!("NON-CONFORMANT IN SAMPLE ({} of {} fail: {})", self.failing, self.judged, why.join(", "));
        }
        format!("CONFORMANT IN SAMPLE ({} reporters)", self.judged)
    }
}

/// A group's measurement as shown: `start 410 ms`, `blamed 12/10k`,
/// `missed targets 1`.
pub fn measure_text(kind: Kind, m: Option<u64>) -> String {
    match (kind, m) {
        (_, None) => "-".into(),
        (Kind::Host, Some(t)) => format!("start {} ms", (t + 5) / 10),
        (Kind::Device, Some(v)) => format!("blamed {v}/10k"),
        (Kind::Perf, Some(v)) => format!("targets missed {v}"),
    }
}

/// The rule, in one paragraph.
pub const RULE: &str = "one report per feed key and identity (the newest); computers with fewer than 4 cores or 4 GiB \
are listed but not judged; a verdict needs 5 judged reporters; NON-CONFORMANT IN SAMPLE when at least 1 in 3 fail \
(halted by R15, or a terse perf requirement or ceiling failed); starved and memory verdicts never count.";

/// The account `terse perf aggregate` prints: one section per sample
/// (`samples`: a title and which reporter keys it admits).
pub fn render(a: &Aggregate, malformed: usize, samples: &[(&str, &dyn Fn(&str) -> bool)]) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "{} reports ({} malformed, ignored)", a.reports + malformed, malformed);
    let _ = writeln!(o, "rule: {RULE}");
    for ((kind, name, version, build), facts) in &a.groups {
        let _ = writeln!(o);
        let _ = writeln!(o, "{} {} {} {}", kind.word(), name, if version.is_empty() { "-" } else { version }, if build.is_empty() { "-" } else { build });
        for (title, keep) in samples {
            let t = tally(facts.iter().filter(|(k, _)| keep(k)).map(|(_, f)| f));
            let _ = writeln!(o, "  {title}: {}", t.verdict());
            let _ = writeln!(
                o,
                "    {} reporters ({} below the floor), failing {} (halted {}, requirement {}, ceiling {}), blamed {}, warned {}, median {}",
                t.reporters,
                t.below,
                t.failing,
                t.halted,
                t.requirement,
                t.ceiling,
                t.blamed,
                t.warned,
                measure_text(*kind, t.median)
            );
        }
    }
    o
}
