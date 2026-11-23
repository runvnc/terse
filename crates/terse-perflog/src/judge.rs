//! Which component made a frame late, and what a host does about it
//! (`PERFORMANCE.md` R14, R15).
//!
//! A host measures, for each frame, how long its work took (from the
//! frame's start to its picture presented) and, for each component, the
//! time the frame loop spent in it or waiting for it, with how much of
//! that the component spent waiting for a CPU (starved: the computer was
//! busy) as the operating system reports it. A component is **late** in a
//! frame only if
//!
//! 1. the frame overran its deadline (one frame, 16.7 ms);
//! 2. it would still have overrun had nobody been starved: the frame's
//!    time less every component's and the frame loop's own starvation is
//!    over the deadline;
//! 3. the system was not stalled on memory for a quarter of a frame or
//!    more (memory pressure: pages being swapped back, nobody's fault),
//!    or the component spent more than its allowance and that stall on a
//!    CPU (a stall excuses its blocked time, but no more of its time on a
//!    CPU than the stall itself: a driver that spins is not excused by
//!    another program's swapping);
//! 4. the component's own time (its time less its starvation: on a CPU,
//!    or blocked, on a disk, a lock, a sleep) is over its allowance, half
//!    a frame, and the most of any component's.
//!
//! The rule (`Rule::default`): after 3 late frames of one component within
//! 60 frames the host warns the user; after 10 within 300 it stops the
//! machines that use it (a device). The host's own components (the core,
//! composing, presenting) cannot be stopped: they are warned about once,
//! until they have gone 300 frames without a late frame. A device is
//! stopped only for blames that are [`certain`]: the system said how long
//! it and the frame loop waited for a CPU.
//!
//! **Resizing.** When the window's surface changes size every driver and
//! the host draw everything again, at the new size, in new buffers. A
//! frame that would be blamed within [`Rule::resize_frames`] frames of a
//! size change (the frame first composed at the new size and the two
//! after it) is recorded as `resize` instead and counts toward nothing,
//! at most [`Rule::resize_budget`] frames within any [`Rule::stop_within`]:
//! a window resized without end does not excuse a slow component for
//! longer than that. Everything here is deterministic given the samples:
//! tests feed it frames.

use std::collections::{HashMap, VecDeque};

/// One frame at 60 frames a second, in nanoseconds.
pub const FRAME_NS: u64 = 1_000_000_000 / 60;

/// A part of a host a frame's time is attributed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Component {
    /// The machines' emulation (the CPU, the System and Machines devices,
    /// and in a host with its devices built in, their calls).
    Core,
    /// Putting the picture together (a renderer, the compositor).
    Compose,
    /// Handing the picture to the display.
    Present,
    /// Time the frame loop spent in none of the above (waiting on
    /// something of its own, its operating system's events).
    Outside,
    /// The device driver serving this slot (its lowest).
    Device(u8),
}

impl Component {
    pub fn is_device(&self) -> bool {
        matches!(self, Component::Device(_))
    }

    /// As the record writes it: `core`, `compose`, `present`, `outside`,
    /// `dev2`.
    pub fn name(&self) -> String {
        match self {
            Component::Core => "core".into(),
            Component::Compose => "compose".into(),
            Component::Present => "present".into(),
            Component::Outside => "outside".into(),
            Component::Device(s) => format!("dev{s}"),
        }
    }

    pub fn parse(s: &str) -> Option<Component> {
        Some(match s {
            "core" => Component::Core,
            "compose" => Component::Compose,
            "present" => Component::Present,
            "outside" => Component::Outside,
            _ => Component::Device(s.strip_prefix("dev")?.parse().ok()?),
        })
    }
}

impl std::fmt::Display for Component {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name())
    }
}

/// One component's share of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub component: Component,
    /// The frame loop's time in this component or waiting for it, in work
    /// it does every frame (a device's BEGIN, calls, END and presenting;
    /// not starting a machine, not a host service, not a snapshot).
    pub wall_ns: u64,
    /// Of that, time its waited-on thread was on a CPU, if known.
    pub run_ns: Option<u64>,
    /// Time its threads were runnable but waited for a CPU, if known.
    pub starved_ns: Option<u64>,
    /// The machine it worked for most in the frame (image ID), if any.
    pub image: Option<[u8; 32]>,
}

impl Part {
    pub fn new(component: Component, wall_ns: u64) -> Part {
        Part { component, wall_ns, run_ns: None, starved_ns: None, image: None }
    }

    /// Starvation, at most the time waited.
    pub fn starved(&self) -> u64 {
        self.starved_ns.unwrap_or(0).min(self.wall_ns)
    }

    /// Its own time: the time waited for it less its starvation.
    pub fn own(&self) -> u64 {
        self.wall_ns - self.starved()
    }

    /// Of its own time, on a CPU (if known; else all of it).
    pub fn cpu(&self) -> u64 {
        self.run_ns.map_or(self.own(), |r| r.min(self.own()))
    }

    /// Of its own time, neither on a CPU nor waiting for one: blocked.
    pub fn blocked(&self) -> u64 {
        self.own() - self.cpu()
    }
}

/// One frame's measurements.
#[derive(Clone, Debug, Default)]
pub struct FrameSample {
    /// The host's frame number (one per frame its loop ran).
    pub frame: u64,
    /// The frame's work: from its start to its picture presented.
    pub wall_ns: u64,
    pub parts: Vec<Part>,
    /// The frame loop's own thread's starvation in the frame, if known.
    pub host_starved_ns: Option<u64>,
    /// The system's full memory stall in the frame (PSI), if known.
    pub memory_stall_ns: Option<u64>,
    /// The frame is the first composed at a new size of the window's
    /// surface (or the first of all, in a window).
    pub size_changed: bool,
}

/// Why a late frame was late.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The component's own doing (conditions 1–4 hold).
    Blamed,
    /// The computer was busy: without the starvation it was on time.
    Starved,
    /// The system was stalled on memory.
    Memory,
    /// No component's own time was over its allowance.
    Spread,
    /// It would have been blamed, but the window's surface had just
    /// changed size (within the rule's bounds).
    Resize,
}

impl Verdict {
    pub fn name(&self) -> &'static str {
        match self {
            Verdict::Blamed => "blamed",
            Verdict::Starved => "starved",
            Verdict::Memory => "memory",
            Verdict::Spread => "spread",
            Verdict::Resize => "resize",
        }
    }

    pub fn parse(s: &str) -> Option<Verdict> {
        Some(match s {
            "blamed" => Verdict::Blamed,
            "starved" => Verdict::Starved,
            "memory" => Verdict::Memory,
            "spread" => Verdict::Spread,
            "resize" => Verdict::Resize,
            _ => return None,
        })
    }
}

/// A frame that overran: its verdict and the component with the most own
/// time (the one blamed, if any).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Late {
    pub frame: u64,
    pub wall_ns: u64,
    pub verdict: Verdict,
    pub top: Part,
}

/// What the host must do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Tell the user `component` is slow (it was late `count` times within
    /// the warning window).
    Warn { component: Component, count: u32 },
    /// Stop the machines that use `component` (a device), after taking
    /// their snapshots. Never for the host's own components.
    Stop { component: Component, count: u32 },
}

/// The numbers of the rule (`PERFORMANCE.md` R15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The frame's deadline.
    pub deadline_ns: u64,
    /// A component's allowance of its own time in a late frame.
    pub allowance_ns: u64,
    /// Memory stall that excuses a frame.
    pub memory_ns: u64,
    pub warn_after: u32,
    pub warn_within: u64,
    pub stop_after: u32,
    pub stop_within: u64,
    /// Frames after a size change of the window's surface (that frame
    /// included) whose blame is excused as `resize`.
    pub resize_frames: u64,
    /// At most this many frames excused as `resize` within `stop_within`.
    pub resize_budget: u32,
}

impl Default for Rule {
    fn default() -> Self {
        Rule {
            deadline_ns: FRAME_NS,
            allowance_ns: FRAME_NS / 2,
            memory_ns: FRAME_NS / 4,
            warn_after: 3,
            warn_within: 60,
            stop_after: 10,
            stop_within: 300,
            resize_frames: 3,
            resize_budget: 120,
        }
    }
}

/// Applies the rule frame by frame.
#[derive(Clone, Debug, Default)]
pub struct Judge {
    pub rule: Rule,
    /// The blames of each component, counted by the rule.
    tally: Tally<Component>,
    /// Frames just after a size change of the window's surface.
    resizes: Resizes,
}

impl Judge {
    pub fn new(rule: Rule) -> Judge {
        Judge { rule, tally: Tally::new(rule), resizes: Resizes::default() }
    }

    /// The verdict on one frame: `None` if it was on time.
    pub fn verdict(&self, s: &FrameSample) -> Option<Late> {
        verdict(&self.rule, s)
    }

    /// Judges one frame: its verdict, and the actions the rule now asks
    /// for. A component is warned about once until it has gone a whole
    /// stop window without blame; after a stop its count starts again. The
    /// host's own components are never stopped (only warned about).
    pub fn judge(&mut self, s: &FrameSample) -> (Option<Late>, Vec<Action>) {
        let mut late = self.verdict(s);
        self.resizes.excuse(&self.rule, s.frame, s.size_changed, &mut late);
        self.tally.rule = self.rule;
        self.tally.tick(s.frame);
        let mut actions = Vec::new();
        if let Some(l) = late.filter(|l| l.verdict == Verdict::Blamed) {
            let c = l.top.component;
            match self.tally.blame(s.frame, c, certain(s, &l.top), c.is_device()) {
                Some(Step::Warn(count)) => actions.push(Action::Warn { component: c, count }),
                Some(Step::Stop(count)) => actions.push(Action::Stop { component: c, count }),
                None => {}
            }
        }
        (late, actions)
    }

    /// How many frames `c` was blamed in within the last `window` frames
    /// before `frame` (inclusive).
    pub fn count(&self, c: Component, frame: u64, window: u64) -> u32 {
        self.tally.count(c, frame, window)
    }
}

/// The verdict on one frame under `r`: `None` if it was on time.
pub fn verdict(r: &Rule, s: &FrameSample) -> Option<Late> {
    if s.wall_ns <= r.deadline_ns {
        return None;
    }
    let top = s.parts.iter().copied().max_by_key(|p| (p.own(), std::cmp::Reverse(p.component)))?;
    let starved: u64 = s.parts.iter().map(Part::starved).sum::<u64>() + s.host_starved_ns.unwrap_or(0);
    let verdict = if s.wall_ns.saturating_sub(starved) <= r.deadline_ns {
        Verdict::Starved
    } else if s.memory_stall_ns.is_some_and(|m| m >= r.memory_ns && cpu_beyond_stall(&top, m) <= r.allowance_ns) {
        Verdict::Memory
    } else if top.own() > r.allowance_ns {
        Verdict::Blamed
    } else {
        Verdict::Spread
    };
    Some(Late { frame: s.frame, wall_ns: s.wall_ns, verdict, top })
}

/// Of `p`'s time on a CPU, what a memory stall of `stall_ns` cannot
/// explain (`RECORD.md` 4, `memory`). The stall is the whole system's,
/// not the component's. Its pages read back from swap leave a component
/// blocked, and the record cannot tell that from any other blocking: a
/// stall may account for all of its blocked time. Memory reclaimed in its
/// own allocations runs on its CPU and is counted in the stall: a stall
/// accounts for no more of its time on a CPU than the stall itself; the
/// rest is its work. This holds for the host's own parts as for drivers
/// and machines: the hosts read their stages' time on a CPU (terse-host's
/// `watch.rs`, terse-bus's `monitor.rs`). Where it is not known (the system
/// gives no reading, `TERSE_PERF_PROBE=none`), 0: the stall excuses it, and
/// the record gives its `cpu-ms` as `-`.
fn cpu_beyond_stall(p: &Part, stall_ns: u64) -> u64 {
    p.run_ns.map_or(0, |_| p.cpu().saturating_sub(stall_ns))
}

/// The excuse for frames just after a size change of the window's surface
/// (`Rule::resize_frames`, at most `Rule::resize_budget` within the stop
/// window): a frame that would be blamed is `resize` instead.
#[derive(Clone, Debug, Default)]
pub struct Resizes {
    /// The last frame first composed at a new surface size.
    size_changed: Option<u64>,
    /// Frames excused as `resize`, oldest first, within `stop_within`.
    excused: VecDeque<u64>,
}

impl Resizes {
    /// Frame `frame` (the first at a new size if `size_changed`) was judged
    /// `late`: a blame within the excuse becomes `resize`.
    pub fn excuse(&mut self, r: &Rule, frame: u64, size_changed: bool, late: &mut Option<Late>) {
        if size_changed {
            self.size_changed = Some(frame);
        }
        while self.excused.front().is_some_and(|&f| f + r.stop_within <= frame) {
            self.excused.pop_front();
        }
        if let Some(l) = late
            && l.verdict == Verdict::Blamed
            && self.size_changed.is_some_and(|c| frame >= c && frame - c < r.resize_frames)
            && (self.excused.len() as u32) < r.resize_budget
        {
            l.verdict = Verdict::Resize;
            self.excused.push_back(frame);
        }
    }
}

/// What the rule asks for after a blame ([`Tally::blame`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Warn: this many blames within the warning window.
    Warn(u32),
    /// Stop: this many certain blames within the stop window.
    Stop(u32),
}

/// The rule's counting (`PERFORMANCE.md` R15), for any key: a component
/// (a driver, the host's own parts), or what a host with built-in devices
/// blames, a machine. Every blame counts towards a warning, only certain
/// ones towards a stop; a key is warned about once until it has gone a
/// whole stop window without blame; after a stop its count starts again.
#[derive(Clone, Debug)]
pub struct Tally<K> {
    pub rule: Rule,
    /// Frames each key was blamed in, oldest first, within the longest
    /// window, and whether the blame was certain.
    blamed: HashMap<K, VecDeque<(u64, bool)>>,
    /// Keys warned about and not yet back to good behaviour: the frame of
    /// their last blame.
    warned: HashMap<K, u64>,
}

impl<K> Default for Tally<K> {
    fn default() -> Self {
        Tally { rule: Rule::default(), blamed: HashMap::new(), warned: HashMap::new() }
    }
}

impl<K: Copy + Eq + std::hash::Hash> Tally<K> {
    pub fn new(rule: Rule) -> Tally<K> {
        Tally { rule, ..Default::default() }
    }

    /// Frame `frame` begins: blames older than every window are forgotten,
    /// and so are warnings about keys a whole stop window without blame.
    pub fn tick(&mut self, frame: u64) {
        let r = self.rule;
        let horizon = r.warn_within.max(r.stop_within);
        for q in self.blamed.values_mut() {
            while q.front().is_some_and(|&(f, _)| f + horizon <= frame) {
                q.pop_front();
            }
        }
        self.blamed.retain(|_, q| !q.is_empty());
        self.warned.retain(|_, &mut last| last + r.stop_within > frame);
    }

    /// `key` was blamed in `frame` (after [`Self::tick`] for that frame).
    /// `can_stop`: false for what cannot be stopped (a host's own
    /// components in terse-bus), which is warned about once, until it has
    /// gone a whole stop window without blame.
    pub fn blame(&mut self, frame: u64, key: K, certain: bool, can_stop: bool) -> Option<Step> {
        let r = self.rule;
        let q = self.blamed.entry(key).or_default();
        q.push_back((frame, certain));
        let within = |w: u64, only_certain: bool| q.iter().filter(|&&(f, sure)| f + w > frame && (sure || !only_certain)).count() as u32;
        let (warn, stop) = (within(r.warn_within, false), within(r.stop_within, true));
        if can_stop && stop >= r.stop_after {
            q.clear();
            self.warned.remove(&key);
            return Some(Step::Stop(stop));
        }
        let step = (warn >= r.warn_after && !self.warned.contains_key(&key)).then_some(Step::Warn(warn));
        if step.is_some() || self.warned.contains_key(&key) {
            self.warned.insert(key, frame);
        }
        step
    }

    /// Forgets `key` (a machine that was stopped, or has gone).
    pub fn forget(&mut self, key: K) {
        self.blamed.remove(&key);
        self.warned.remove(&key);
    }

    /// How many frames `key` was blamed in within the last `window` frames
    /// before `frame` (inclusive).
    pub fn count(&self, key: K, frame: u64, window: u64) -> u32 {
        self.blamed.get(&key).map_or(0, |q| q.iter().filter(|&&(f, _)| f + window > frame).count() as u32)
    }
}

/// Whether blaming `top` in frame `s` is certain enough to stop machines
/// for (`RECORD.md` section 4): for a device, the operating system said
/// how long its threads and the frame loop's waited for a CPU (or, on
/// Windows, the wait was inferred from their CPU time and the system's
/// idle time, [`crate::probe::infer`]), so that time on a busy computer
/// was not taken for its own. Where it cannot (Linux without schedstats;
/// Windows where the cycle counts cannot be read; readings turned off), a
/// device is blamed by its waits alone: warned about and recorded, never
/// stopped.
/// The host's own components are never stopped anyway.
pub fn certain(s: &FrameSample, top: &Part) -> bool {
    !top.component.is_device() || (top.starved_ns.is_some() && s.host_starved_ns.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    fn frame(n: u64, wall: u64, parts: &[Part]) -> FrameSample {
        FrameSample { frame: n, wall_ns: wall, parts: parts.to_vec(), host_starved_ns: Some(0), memory_stall_ns: Some(0), size_changed: false }
    }

    fn dev(slot: u8, wall: u64, run: u64, starved: u64) -> Part {
        Part { component: Component::Device(slot), wall_ns: wall, run_ns: Some(run), starved_ns: Some(starved), image: None }
    }

    #[test]
    fn verdicts() {
        let j = Judge::default();
        // On time, whatever the parts.
        assert_eq!(j.verdict(&frame(1, 16 * MS, &[dev(2, 15 * MS, 0, 0)])), None);
        // Blocked 40 ms in the driver: blamed, all of it blocked.
        let l = j.verdict(&frame(2, 42 * MS, &[dev(2, 40 * MS, MS, 0), dev(3, MS, MS, 0)])).unwrap();
        assert_eq!((l.verdict, l.top.component, l.top.blocked(), l.top.cpu()), (Verdict::Blamed, Component::Device(2), 39 * MS, MS));
        // The same wait, but 30 ms of it waiting for a CPU: the frame
        // would have been on time.
        let l = j.verdict(&frame(3, 42 * MS, &[dev(2, 40 * MS, 5 * MS, 30 * MS)])).unwrap();
        assert_eq!(l.verdict, Verdict::Starved);
        // Starvation of the frame loop itself counts too.
        let mut f = frame(4, 42 * MS, &[dev(2, 20 * MS, 5 * MS, 0)]);
        f.host_starved_ns = Some(30 * MS);
        assert_eq!(j.verdict(&f).unwrap().verdict, Verdict::Starved);
        // Memory pressure excuses blocked time.
        let mut f = frame(5, 42 * MS, &[dev(2, 40 * MS, 0, 0)]);
        f.memory_stall_ns = Some(5 * MS);
        assert_eq!(j.verdict(&f).unwrap().verdict, Verdict::Memory);
        // But not time on a CPU beyond the stall: 40 ms spinning in a
        // frame in which the system stalled 5 ms (another program
        // swapping) is the driver's doing; so is 18 ms with 9 ms of stall.
        let spin = |cpu: u64, stall: u64| {
            let mut f = frame(5, 42 * MS, &[dev(2, 40 * MS, cpu, 0)]);
            f.memory_stall_ns = Some(stall);
            j.verdict(&f).unwrap().verdict
        };
        assert_eq!(spin(40 * MS, 5 * MS), Verdict::Blamed);
        assert_eq!(spin(18 * MS, 9 * MS), Verdict::Blamed);
        // The stall may explain as much CPU time as it lasted (reclaim in
        // the driver's own allocations): within the allowance once that
        // is taken off, excused.
        assert_eq!(spin(16 * MS, 8 * MS), Verdict::Memory);
        assert_eq!(spin(40 * MS, 35 * MS), Verdict::Memory);
        // Below a quarter of a frame a stall excuses nothing.
        assert_eq!(spin(0, 4 * MS), Verdict::Blamed);
        // Where the CPU time is not known, the stall excuses as before.
        let mut f = frame(5, 42 * MS, &[Part { run_ns: None, ..dev(2, 40 * MS, 0, 0) }]);
        f.memory_stall_ns = Some(5 * MS);
        assert_eq!(j.verdict(&f).unwrap().verdict, Verdict::Memory);
        // Late, but nobody over half a frame.
        let l = j.verdict(&frame(6, 20 * MS, &[dev(2, 6 * MS, 6 * MS, 0), Part::new(Component::Core, 7 * MS)])).unwrap();
        assert_eq!((l.verdict, l.top.component), (Verdict::Spread, Component::Core));
        // Unknown starvation (no schedstat): blamed by the wait alone.
        let l = j.verdict(&frame(7, 30 * MS, &[Part::new(Component::Device(7), 25 * MS)])).unwrap();
        assert_eq!(l.verdict, Verdict::Blamed);
    }

    /// The host's own parts have their time on a CPU read like any
    /// other's: a stage that spins through a frame in which the system
    /// stalled on memory is blamed, one that was blocked is excused.
    #[test]
    fn a_memory_stall_excuses_a_host_stage_no_more_than_a_driver() {
        let j = Judge::default();
        let stage = |c: Component, own: u64, run: Option<u64>, stall: u64| {
            let mut f = frame(9, own + MS, &[Part { run_ns: run, starved_ns: Some(0), ..Part::new(c, own) }, Part::new(Component::Outside, MS)]);
            f.memory_stall_ns = Some(stall);
            j.verdict(&f).unwrap()
        };
        // As recorded on a computer short of memory (`TERSE_HOST_SLOW=
        // compose:30:0:spin`): the drawing 30.4 ms on a CPU, the system
        // stalled 6 ms. Its doing.
        for c in [Component::Compose, Component::Core, Component::Present, Component::Outside] {
            let l = stage(c, 30_400_000, Some(30_400_000), 6 * MS);
            assert_eq!((l.verdict, l.top.component, l.top.cpu(), l.top.blocked()), (Verdict::Blamed, c, 30_400_000, 0), "{c}");
            // The same stage blocked (its pages read back): excused.
            assert_eq!(stage(c, 30_400_000, Some(MS), 6 * MS).verdict, Verdict::Memory, "{c}");
            // On a CPU no more than the allowance and the stall: excused.
            assert_eq!(stage(c, 30 * MS, Some(14 * MS), 6 * MS).verdict, Verdict::Memory, "{c}");
            assert_eq!(stage(c, 30 * MS, Some(15 * MS), 6 * MS).verdict, Verdict::Blamed, "{c}");
        }
        // A process's CPU time over a stage drawn on many threads may be
        // more than the stage's time: it counts as on a CPU all of it.
        let l = stage(Component::Compose, 30 * MS, Some(200 * MS), 20 * MS);
        assert_eq!((l.verdict, l.top.cpu()), (Verdict::Blamed, 30 * MS));
        // Only where no reading was had does a stall excuse it whole.
        assert_eq!(stage(Component::Compose, 30_400_000, None, 6 * MS).verdict, Verdict::Memory);
    }

    #[test]
    fn warns_after_3_in_60_stops_after_10_in_300() {
        let mut j = Judge::default();
        let slow = |n| frame(n, 50 * MS, &[dev(2, 45 * MS, 45 * MS, 0)]);
        let good = |n| frame(n, 2 * MS, &[dev(2, MS, MS, 0)]);
        let mut actions = Vec::new();
        // A late frame every 10 frames: 3 within 60 by frame 20, 10 by 90;
        // then the count starts again (a warning at 120, ...).
        for n in 0..100u64 {
            let (_, a) = j.judge(&if n % 10 == 0 { slow(n) } else { good(n) });
            actions.extend(a.into_iter().map(|a| (n, a)));
        }
        let c = Component::Device(2);
        assert_eq!(actions, vec![(20, Action::Warn { component: c, count: 3 }), (90, Action::Stop { component: c, count: 10 })]);
        // One late frame every 40 never warns (2 within 60) nor stops (8
        // within 300): a hiccup now and then is tolerated.
        let mut j = Judge::default();
        for n in 0..3000u64 {
            let (_, a) = j.judge(&if n % 40 == 0 { slow(n) } else { good(n) });
            assert!(a.is_empty(), "{n}: {a:?}");
        }
        // One every 30, sustained, stops it: 10 within 300.
        let mut j = Judge::default();
        let stops: Vec<u64> = (0..1000u64).filter(|&n| j.judge(&if n % 30 == 0 { slow(n) } else { good(n) }).1.iter().any(|a| matches!(a, Action::Stop { .. }))).collect();
        assert_eq!(stops, vec![270, 570, 870]);
    }

    #[test]
    fn without_starvation_readings_a_device_is_warned_about_never_stopped() {
        // Linux without schedstats, Windows without its cycle counts: the
        // driver's wait is all the host knows.
        let unknown = |n| FrameSample { frame: n, wall_ns: 50 * MS, parts: vec![Part::new(Component::Device(2), 45 * MS)], host_starved_ns: None, memory_stall_ns: None, size_changed: false };
        let mut j = Judge::default();
        let mut actions = Vec::new();
        for n in 0..600u64 {
            let (late, a) = j.judge(&unknown(n));
            assert_eq!(late.unwrap().verdict, Verdict::Blamed);
            actions.extend(a.into_iter().map(|a| (n, a)));
        }
        assert_eq!(actions, vec![(2, Action::Warn { component: Component::Device(2), count: 3 })]);
        // The driver's starvation read but not the frame loop's (macOS
        // gives both, but a system might give one): still uncertain.
        let mut j = Judge::default();
        for n in 0..600u64 {
            let mut f = frame(n, 50 * MS, &[dev(2, 45 * MS, 45 * MS, 0)]);
            f.host_starved_ns = None;
            assert!(j.judge(&f).1.iter().all(|a| matches!(a, Action::Warn { .. })));
        }
    }

    /// Windows: the waits inferred from CPU time and idle time
    /// (`probe::infer`) make blame certain where the facts allow it.
    #[test]
    fn inferred_waits_blame_with_certainty() {
        use crate::probe::infer;
        // A frame of 50 ms on 8 processors: the core waited 45 ms for the
        // driver, which used `cpu` of CPU time in it; the processors were
        // idle `idle` in all; the frame loop ran its own 5 ms.
        let frame_of = |n: u64, cpu: u64, idle: u64| {
            let starved = infer(45 * MS, cpu, idle * 45 / 50, 8).starved_ns;
            let host = infer(5 * MS, 5 * MS, idle * 5 / 50, 8).starved_ns;
            FrameSample {
                frame: n,
                wall_ns: 50 * MS,
                parts: vec![Part { component: Component::Device(2), wall_ns: 45 * MS, run_ns: Some(cpu), starved_ns: Some(starved), image: None }],
                host_starved_ns: Some(host),
                memory_stall_ns: None,
                size_changed: false,
            }
        };
        let stops = |cpu: u64, idle: u64| {
            let mut j = Judge::default();
            let mut verdicts = Vec::new();
            let mut stopped = None;
            for n in 0..20 {
                let (late, a) = j.judge(&frame_of(n, cpu, idle));
                verdicts.push(late.unwrap().verdict);
                if stopped.is_none() && a.iter().any(|a| matches!(a, Action::Stop { .. })) {
                    stopped = Some(n);
                }
            }
            (verdicts[0], stopped)
        };
        // Spinning (40 ms of CPU in 45), even on a computer with no
        // processor idle: its 5 ms off a CPU may have been a wait for one
        // (as Linux would read it), its 40 ms on one are its own: blamed,
        // certain, stopped at the 10th. So with 20 ms on a CPU: 25 ms
        // excused, 20 its own.
        assert_eq!(stops(40 * MS, 0), (Verdict::Blamed, Some(9)));
        assert_eq!(stops(20 * MS, 0), (Verdict::Blamed, Some(9)));
        assert_eq!(infer(45 * MS, 40 * MS, 0, 8).starved_ns, 5 * MS);
        // With a processor to spare, its time off a CPU is its own.
        assert_eq!(infer(45 * MS, 40 * MS, 45 * MS, 8).starved_ns, 0);
        // Sleeping (1 ms of CPU) with processors to spare: blocked on
        // something of its own, blamed, certain: stopped.
        assert_eq!(stops(MS, 200 * MS), (Verdict::Blamed, Some(9)));
        // Sleeping, or wanting a CPU, on a computer with no processor to
        // spare: starved, nobody blamed.
        assert_eq!(stops(MS, 10 * MS), (Verdict::Starved, None));
    }

    #[test]
    fn starved_frames_are_never_counted() {
        let mut j = Judge::default();
        for n in 0..600 {
            let (late, a) = j.judge(&frame(n, 60 * MS, &[dev(2, 58 * MS, 2 * MS, 50 * MS)]));
            assert_eq!(late.unwrap().verdict, Verdict::Starved);
            assert!(a.is_empty());
        }
    }

    #[test]
    fn warned_once_until_it_behaves() {
        let mut j = Judge::default();
        let slow = |n| frame(n, 50 * MS, &[dev(4, 45 * MS, 0, 0)]);
        let mut warns = Vec::new();
        for n in [0, 1, 2, 3, 4, 5, 6, 7, 8, 400, 401, 402] {
            for a in j.judge(&slow(n)).1 {
                warns.push((n, a));
            }
        }
        let c = Component::Device(4);
        assert_eq!(
            warns,
            vec![(2, Action::Warn { component: c, count: 3 }), (400 + 2, Action::Warn { component: c, count: 3 })]
        );
    }

    #[test]
    fn the_hosts_own_components_are_warned_about_once_never_stopped() {
        // The host's compose late in every frame for ten seconds: one
        // warning, at the third, then nothing while it goes on.
        let mut j = Judge::default();
        let mut actions = Vec::new();
        for n in 0..600u64 {
            let (late, a) = j.judge(&frame(n, 30 * MS, &[Part::new(Component::Compose, 25 * MS)]));
            assert_eq!(late.unwrap().verdict, Verdict::Blamed);
            actions.extend(a.into_iter().map(|a| (n, a)));
        }
        assert_eq!(actions, vec![(2, Action::Warn { component: Component::Compose, count: 3 })]);
        // After 300 good frames it is warned about again.
        let good = |n| frame(n, 2 * MS, &[Part::new(Component::Compose, MS)]);
        for n in 600..900u64 {
            assert!(j.judge(&good(n)).1.is_empty());
        }
        let a: Vec<Action> = (900..903u64).flat_map(|n| j.judge(&frame(n, 30 * MS, &[Part::new(Component::Compose, 25 * MS)])).1).collect();
        assert_eq!(a, vec![Action::Warn { component: Component::Compose, count: 3 }]);
    }

    /// `f` with a size change of the window's surface.
    fn resized(mut f: FrameSample) -> FrameSample {
        f.size_changed = true;
        f
    }

    #[test]
    fn frames_after_a_resize_are_excused() {
        let slow = |n| frame(n, 40 * MS, &[dev(3, 30 * MS, 30 * MS, 0)]);
        let good = |n| frame(n, 2 * MS, &[dev(3, MS, MS, 0)]);
        // A resize every 30 frames, the driver late in the three frames
        // after each: all excused, nothing counted, for ten seconds.
        let mut j = Judge::default();
        for n in 0..600u64 {
            let f = match n % 30 {
                0 => resized(slow(n)),
                1 | 2 => slow(n),
                _ => good(n),
            };
            let (late, a) = j.judge(&f);
            assert!(a.is_empty(), "{n}: {a:?}");
            if n % 30 < 3 {
                assert_eq!(late.unwrap().verdict, Verdict::Resize, "{n}");
            }
        }
        // The fourth frame after a resize is not excused.
        let mut j = Judge::default();
        let verdicts: Vec<Verdict> = (0..5u64).map(|n| j.judge(&if n == 0 { resized(slow(n)) } else { slow(n) }).0.unwrap().verdict).collect();
        assert_eq!(verdicts, [Verdict::Resize, Verdict::Resize, Verdict::Resize, Verdict::Blamed, Verdict::Blamed]);
        // Starved and spread frames keep their verdicts.
        let mut j = Judge::default();
        assert_eq!(j.judge(&resized(good(0))).0, None);
        let f = resized(frame(1, 60 * MS, &[dev(3, 58 * MS, 2 * MS, 50 * MS)]));
        assert_eq!(j.judge(&f).0.unwrap().verdict, Verdict::Starved);
        let f = resized(frame(2, 20 * MS, &[dev(3, 6 * MS, 6 * MS, 0)]));
        assert_eq!(j.judge(&f).0.unwrap().verdict, Verdict::Spread);
    }

    #[test]
    fn a_slow_device_is_stopped_however_the_window_is_resized() {
        // The surface changes size in every frame (a resize that never
        // ends) and the driver is late in every frame: 120 frames are
        // excused, then it is warned (3 within 60) and stopped (10 within
        // 300), 120 frames later than without the resizing; then again 10
        // frames later, the budget still spent.
        let c = Component::Device(3);
        let late = |n| frame(n, 40 * MS, &[dev(3, 30 * MS, 30 * MS, 0)]);
        let mut j = Judge::default();
        let actions: Vec<(u64, Action)> = (0..140u64).flat_map(|n| j.judge(&resized(late(n))).1.into_iter().map(move |a| (n, a))).collect();
        assert_eq!(
            actions,
            [
                (122, Action::Warn { component: c, count: 3 }),
                (129, Action::Stop { component: c, count: 10 }),
                (132, Action::Warn { component: c, count: 3 }),
                (139, Action::Stop { component: c, count: 10 })
            ]
        );
        let mut j = Judge::default();
        let actions: Vec<(u64, Action)> = (0..10u64).flat_map(|n| j.judge(&late(n)).1.into_iter().map(move |a| (n, a))).collect();
        assert_eq!(actions, [(2, Action::Warn { component: c, count: 3 }), (9, Action::Stop { component: c, count: 10 })]);
        // The budget comes back as the excused frames leave the window.
        let mut j = Judge::default();
        let mut excused = 0;
        for n in 0..1200u64 {
            let (l, _) = j.judge(&resized(late(n)));
            if l.unwrap().verdict == Verdict::Resize {
                excused += 1;
            }
        }
        assert_eq!(excused, 4 * 120);
    }

    #[test]
    fn names() {
        for c in [Component::Core, Component::Compose, Component::Present, Component::Outside, Component::Device(9)] {
            assert_eq!(Component::parse(&c.name()), Some(c));
        }
        assert_eq!(Component::parse("dev"), None);
    }
}
