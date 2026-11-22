//! The rule, acting in a host whose devices are built in (`PERFORMANCE.md`
//! R14, R15; `RECORD.md` 4): who a late frame is blamed on, and what the
//! host does about it. Everything here is deterministic given the frames'
//! measurements, so tests feed it frames; `watch.rs` measures them and
//! `app.rs` carries out what it asks.
//!
//! **Who.** terse-host runs every machine and every device on its frame
//! loop thread (and draws with its renderer's threads), so it cannot time
//! separate processes. It times each machine's share of the frame instead:
//! the time the frame loop spent on that machine's frame, that is its
//! emulation, the calls of its built-in devices made within it (a vector
//! list built, the terminal written, a page given, sound mixed), and the
//! host-side work of those devices for it after the frame (its page's
//! layouts taken in and asked for, its content requests handed over), with
//! the frame loop thread's CPU time and run-queue wait read around it. The
//! rest is the host's own, shared by every machine: `core` (the frame's
//! work besides the machines: completions delivered, sound sent to the
//! output), `compose` (taking the screens and drawing the picture, the
//! renderer's threads included), `present` (handing it to the display) and
//! `outside` (how late the frame began after its tick or the last picture: the event loop held
//! elsewhere). A part is blamed as `RECORD.md` 4 says: the frame overran,
//! not because anybody waited for a CPU, not in a memory stall, and the
//! part's own time is over half a frame and the most of any.
//!
//! - A **machine** blamed: the machine. Its devices are the host's own, so
//!   the record names the component `core` and the machine by its program
//!   (the `late` line's image, the `stop` line's handle).
//! - **compose**: what a machine can ask of the drawing is bounded by the
//!   caps (R10) and the system ink (R12), so a slow picture is the host's
//!   drawing on this computer, never a machine's doing. First the host
//!   sheds the optional look (one level per blame, `shed ... late`); with
//!   nothing left to shed it warns, and after the stop count it says plainly
//!   that it does not keep up here (`fail`), with no machine stopped.
//! - **core**, **present**: the same, without the shedding.
//! - **outside**: recorded and warned about; never more (not the host's
//!   work, nor anybody's it can name).
//! - **starved**, **memory**, **spread**: nobody.
//!
//! **Certain.** As for a driver (`RECORD.md` 4), a blame stops a machine
//! only if the operating system said how long the blamed part waited for a
//! CPU: the frame loop thread's run-queue wait for a machine, `core`,
//! `present` and `outside` (Linux with schedstats; macOS, as the process's
//! wait, which is at least the thread's), and for `compose` also the
//! process's (its drawing threads'). Windows gives neither: there a blame
//! warns, never stops.
//!
//! **Excused frames.** Frames right after the window appears, after its
//! surface really changes size, after it moves to another monitor or its
//! scale changes, and a machine's first two frames, are judged and
//! recorded with their reason (`excused`) and not counted: they redo work
//! a steady frame does not (a surface, glyphs at a new size, caches). At
//! most [`EXCUSED_MAX`] late frames within any [`EXCUSE_WINDOW`] frames are
//! excused, so continuous resizing cannot hide a slow host: beyond that its
//! late frames count.

use std::collections::{HashMap, VecDeque};

use terse_perflog::{Component, FrameSample, Late, Rule, Step, Tally, Verdict};

/// Who a part of a frame is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Culprit {
    /// A machine, by its serial (0: the shell, or the machine alone).
    Machine(u32),
    /// The host's own shared work.
    Host(Component),
}

/// Why frames are excused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Excuse {
    /// The run's first frames.
    Start,
    /// A machine's first frames.
    Open,
    /// The picture's size changed.
    Size,
    /// Another monitor, or its scale.
    Monitor,
}

impl Excuse {
    pub fn name(&self) -> &'static str {
        match self {
            Excuse::Start => "start",
            Excuse::Open => "open",
            Excuse::Size => "size",
            Excuse::Monitor => "monitor",
        }
    }

    /// Frames excused after it: the start a half second (the first
    /// glyphs, pipelines, the first page layouts), a change of size or
    /// monitor a sixth (a new surface, glyphs at the new size).
    pub fn frames(&self) -> u64 {
        match self {
            Excuse::Start => 30,
            Excuse::Open => 2,
            Excuse::Size | Excuse::Monitor => 10,
        }
    }
}

/// At most this many late frames are excused within [`EXCUSE_WINDOW`].
pub const EXCUSED_MAX: usize = 60;
/// The window of [`EXCUSED_MAX`]: the rule's stop window, five seconds.
pub const EXCUSE_WINDOW: u64 = 300;
/// A machine's first frames, excused ([`Excuse::Open`]).
pub const OPEN_FRAMES: u64 = 2;

/// What the host must do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Shed one more level of the look (the rule's first step).
    Shed,
    /// Tell the user; for a machine, save it at once.
    Warn { culprit: Culprit, count: u32 },
    /// A machine: save it and stop it (halted). The host's own `core`,
    /// `compose` or `present`: say it does not keep up here (`fail`).
    Stop { culprit: Culprit, count: u32 },
}

/// One frame judged.
#[derive(Clone, Debug, PartialEq)]
pub struct Judged {
    /// The verdict, if the frame was late.
    pub late: Option<Late>,
    /// Who the frame's top part is.
    pub culprit: Option<Culprit>,
    /// Why the frame was not counted, if it was excused.
    pub excused: Option<Excuse>,
    pub acts: Vec<Act>,
}

/// The rule.
#[derive(Debug)]
pub struct Enforcer {
    pub rule: Rule,
    tally: Tally<Culprit>,
    /// Frames before this one are excused, and why.
    until: Option<(u64, Excuse)>,
    /// The frames whose lateness was excused, within the window.
    excused: VecDeque<u64>,
}

impl Default for Enforcer {
    fn default() -> Self {
        Enforcer::new(Rule::default())
    }
}

impl Enforcer {
    pub fn new(rule: Rule) -> Enforcer {
        Enforcer { rule, tally: Tally::new(rule), until: None, excused: VecDeque::new() }
    }

    /// Something happened before `frame` that excuses it and the frames
    /// after it (`why.frames()` in all).
    pub fn excuse(&mut self, frame: u64, why: Excuse) {
        let end = frame + why.frames();
        if self.until.is_none_or(|(u, _)| u <= end) {
            self.until = Some((end, why));
        }
    }

    /// Forgets a machine's blames (it was stopped, or has gone).
    pub fn forget(&mut self, culprit: Culprit) {
        self.tally.forget(culprit);
    }

    /// Judges frame `s`, whose part `i` is `who[i]`; `opened[i]`: that part
    /// is a machine in its first frames. `can_shed`: the look has a level
    /// left to shed.
    pub fn judge(&mut self, s: &FrameSample, who: &[Culprit], opened: &[bool], can_shed: bool) -> Judged {
        let r = self.rule;
        self.tally.tick(s.frame);
        while self.excused.front().is_some_and(|&f| f + EXCUSE_WINDOW <= s.frame) {
            self.excused.pop_front();
        }
        let late = terse_perflog::judge::verdict(&r, s);
        let mut out = Judged { late, culprit: None, excused: None, acts: Vec::new() };
        let Some(l) = late else { return out };
        let i = s.parts.iter().position(|p| *p == l.top);
        let culprit = i.and_then(|i| who.get(i).copied());
        out.culprit = culprit;
        let reason = match self.until {
            Some((u, why)) if s.frame < u => Some(why),
            _ => i.and_then(|i| opened.get(i)).is_some_and(|&o| o).then_some(Excuse::Open),
        };
        if let Some(why) = reason
            && self.excused.len() < EXCUSED_MAX
        {
            self.excused.push_back(s.frame);
            out.excused = Some(why);
            return out;
        }
        let Some(c) = culprit.filter(|_| l.verdict == Verdict::Blamed) else { return out };
        // The first step: shed the optional look, instead of counting.
        if c == Culprit::Host(Component::Compose) && can_shed {
            out.acts.push(Act::Shed);
            return out;
        }
        let certain = l.top.starved_ns.is_some() && s.host_starved_ns.is_some();
        match self.tally.blame(s.frame, c, certain) {
            Some(Step::Warn(count)) => out.acts.push(Act::Warn { culprit: c, count }),
            // The event loop held elsewhere is nobody the host can stop or
            // fault: it warns again.
            Some(Step::Stop(count)) if c == Culprit::Host(Component::Outside) => out.acts.push(Act::Warn { culprit: c, count }),
            Some(Step::Stop(count)) => out.acts.push(Act::Stop { culprit: c, count }),
            None => {}
        }
        out
    }
}

/// How many frames each machine has run (for [`Excuse::Open`]), by serial.
#[derive(Debug, Default)]
pub struct Opened(HashMap<u32, u64>);

impl Opened {
    /// Machine `serial` ran a frame: whether it is one of its first.
    pub fn ran(&mut self, serial: u32) -> bool {
        let n = self.0.entry(serial).or_default();
        *n += 1;
        *n <= OPEN_FRAMES
    }

    /// Machine `serial` starts again (restored, or a new machine reusing
    /// nothing: serials are not reused, but a restore starts afresh).
    pub fn reset(&mut self, serial: u32) {
        self.0.remove(&serial);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terse_perflog::Part;

    const MS: u64 = 1_000_000;

    /// A frame: the shell's part, machine 5's (serial 5), compose, present.
    fn frame(n: u64, machine: u64, compose: u64, starved: u64, readings: bool) -> (FrameSample, Vec<Culprit>) {
        let part = |c, wall, st: u64| Part { component: c, wall_ns: wall, run_ns: None, starved_ns: readings.then_some(st), image: None };
        let mut m = part(Component::Core, machine, starved);
        m.image = Some([5; 32]);
        let parts = vec![part(Component::Core, MS, 0), m, part(Component::Compose, compose, 0), part(Component::Present, MS / 2, 0)];
        let wall = parts.iter().map(|p| p.wall_ns).sum();
        let who = vec![Culprit::Machine(0), Culprit::Machine(5), Culprit::Host(Component::Compose), Culprit::Host(Component::Present)];
        (FrameSample { frame: n, wall_ns: wall, parts, host_starved_ns: readings.then_some(0), memory_stall_ns: Some(0) }, who)
    }

    fn run(e: &mut Enforcer, frames: impl Iterator<Item = (FrameSample, Vec<Culprit>)>, can_shed: bool) -> Vec<(u64, Act)> {
        let mut out = Vec::new();
        for (s, who) in frames {
            let n = s.frame;
            out.extend(e.judge(&s, &who, &[false; 4], can_shed).acts.into_iter().map(|a| (n, a)));
        }
        out
    }

    #[test]
    fn a_slow_machine_is_warned_about_then_stopped_and_nobody_else() {
        let mut e = Enforcer::default();
        let m = Culprit::Machine(5);
        // Machine 5 takes 30 ms from frame 10 on.
        let acts = run(&mut e, (0..20).map(|n| frame(n, if n >= 10 { 30 * MS } else { 2 * MS }, 3 * MS, 0, true)), true);
        assert_eq!(acts, vec![(12, Act::Warn { culprit: m, count: 3 }), (19, Act::Stop { culprit: m, count: 10 })]);
    }

    #[test]
    fn slow_drawing_sheds_first_then_warns_then_fails_never_stops_a_machine() {
        let mut e = Enforcer::default();
        let c = Culprit::Host(Component::Compose);
        // Three levels to shed, then nothing: the frames stay slow.
        let mut floor = 0;
        let mut acts = Vec::new();
        for n in 0..23 {
            let (s, who) = frame(n, 2 * MS, 25 * MS, 0, true);
            for a in e.judge(&s, &who, &[false; 4], floor < 3).acts {
                if a == Act::Shed {
                    floor += 1;
                }
                acts.push((n, a));
            }
        }
        assert_eq!(
            acts,
            vec![(0, Act::Shed), (1, Act::Shed), (2, Act::Shed), (5, Act::Warn { culprit: c, count: 3 }), (12, Act::Stop { culprit: c, count: 10 }), (15, Act::Warn { culprit: c, count: 3 }), (22, Act::Stop { culprit: c, count: 10 })]
        );
        assert!(acts.iter().all(|(_, a)| !matches!(a, Act::Warn { culprit: Culprit::Machine(_), .. } | Act::Stop { culprit: Culprit::Machine(_), .. })));
    }

    #[test]
    fn a_starved_frame_blames_nobody() {
        let mut e = Enforcer::default();
        // Machine 5 waited 25 of its 30 ms for a CPU.
        for n in 0..600 {
            let (s, who) = frame(n, 30 * MS, 3 * MS, 25 * MS, true);
            let j = e.judge(&s, &who, &[false; 4], true);
            assert_eq!(j.late.unwrap().verdict, Verdict::Starved);
            assert!(j.acts.is_empty());
        }
    }

    #[test]
    fn without_wait_readings_a_machine_is_warned_about_never_stopped() {
        // Windows, TERSE_PERF_PROBE=none: blamed by its time alone.
        let mut e = Enforcer::default();
        let acts = run(&mut e, (0..600).map(|n| frame(n, 30 * MS, 3 * MS, 0, false)), true);
        assert_eq!(acts, vec![(2, Act::Warn { culprit: Culprit::Machine(5), count: 3 })]);
        // The same for the host's drawing: shed, warned about, no `fail`.
        let mut e = Enforcer::default();
        let acts = run(&mut e, (0..600).map(|n| frame(n, 2 * MS, 25 * MS, 0, false)), false);
        assert_eq!(acts, vec![(2, Act::Warn { culprit: Culprit::Host(Component::Compose), count: 3 })]);
    }

    #[test]
    fn excused_frames_and_their_bound() {
        // A resize before frame 100: frames 100 to 109 are excused.
        let mut e = Enforcer::default();
        let slow = |n| frame(n, 30 * MS, 3 * MS, 0, true);
        e.excuse(100, Excuse::Size);
        let mut excused = Vec::new();
        let mut acts = Vec::new();
        for n in 100..115 {
            let (s, who) = slow(n);
            let j = e.judge(&s, &who, &[false; 4], true);
            if let Some(w) = j.excused {
                excused.push((n, w));
            }
            acts.extend(j.acts.into_iter().map(|a| (n, a)));
        }
        assert_eq!(excused, (100..110).map(|n| (n, Excuse::Size)).collect::<Vec<_>>());
        assert_eq!(acts, vec![(112, Act::Warn { culprit: Culprit::Machine(5), count: 3 })]);
        // Resizing without end: a resize before every frame excuses at
        // most 60 frames in 300; then the late frames count, and a slow
        // machine is stopped all the same.
        let mut e = Enforcer::default();
        let mut excused = 0;
        let mut stop = None;
        for n in 0..300 {
            e.excuse(n, Excuse::Size);
            let (s, who) = slow(n);
            let j = e.judge(&s, &who, &[false; 4], true);
            excused += j.excused.is_some() as usize;
            if stop.is_none() && j.acts.iter().any(|a| matches!(a, Act::Stop { .. })) {
                stop = Some(n);
            }
        }
        assert_eq!((excused, stop), (EXCUSED_MAX, Some(EXCUSED_MAX as u64 + 9)));
        // A machine's first frames are excused for it alone.
        let mut e = Enforcer::default();
        let (s, who) = slow(5);
        let j = e.judge(&s, &who, &[false, true, false, false], true);
        assert_eq!((j.excused, j.culprit), (Some(Excuse::Open), Some(Culprit::Machine(5))));
        let (s, who) = frame(6, 2 * MS, 25 * MS, 0, true);
        assert_eq!(e.judge(&s, &who, &[false, true, false, false], true).excused, None, "not the drawing");
    }

    #[test]
    fn outside_is_warned_about_never_failed() {
        let mut e = Enforcer::default();
        let mut acts = Vec::new();
        for n in 0..20 {
            let (mut s, mut who) = frame(n, 2 * MS, 3 * MS, 0, true);
            s.parts.push(Part { component: Component::Outside, wall_ns: 30 * MS, run_ns: None, starved_ns: Some(0), image: None });
            s.wall_ns += 30 * MS;
            who.push(Culprit::Host(Component::Outside));
            acts.extend(e.judge(&s, &who, &[false; 5], true).acts.into_iter().map(|a| (n, a)));
        }
        let o = Culprit::Host(Component::Outside);
        assert_eq!(acts, vec![(2, Act::Warn { culprit: o, count: 3 }), (9, Act::Warn { culprit: o, count: 10 }), (12, Act::Warn { culprit: o, count: 3 }), (19, Act::Warn { culprit: o, count: 10 })]);
    }

    #[test]
    fn opened() {
        let mut o = Opened::default();
        assert_eq!([o.ran(3), o.ran(3), o.ran(3)], [true, true, false]);
        o.reset(3);
        assert!(o.ran(3));
    }
}
