//! Holding the machines while the window's size changes (PERFORMANCE.md
//! R15, "Resizing"), shared by terse-host and terse-bus: while the surface's
//! size keeps changing no machine frame runs and nothing is drawn again at
//! each size; the last picture is shown scaled to the window, until the
//! size has kept still for [`SETTLE`] frame periods, for at most
//! [`MAX_HOLD`] in a row, and not again until [`RUN_BETWEEN`] frames have
//! run since. The first frame after a hold runs and is drawn at the size
//! the window has then, whatever the machines do (an idle program's
//! picture is drawn again too: the hold forgets what was presented).

/// Frame periods the size must keep still before frames run again.
pub const SETTLE: u64 = 6;
/// Frame periods held in a row at most...
pub const MAX_HOLD: u64 = 60;
/// ...and frames run before the next hold.
pub const RUN_BETWEEN: u64 = 60;

/// The state of holding, frame period by frame period.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResizeHold {
    /// The tick of the last change of size.
    resized: Option<u64>,
    /// Frame periods held in a row, and frames run since the last hold.
    held_run: u64,
    since_hold: u64,
    /// Frame periods held in all.
    pub held: u64,
}

impl Default for ResizeHold {
    fn default() -> Self {
        ResizeHold { resized: None, held_run: 0, since_hold: RUN_BETWEEN, held: 0 }
    }
}

impl ResizeHold {
    /// The surface took a different size during tick `tick` (before the
    /// next call of [`ResizeHold::step`]).
    pub fn resized(&mut self, tick: u64) {
        self.resized = Some(tick);
    }

    /// Whether frame period `tick` (one more than the last) is held.
    pub fn step(&mut self, tick: u64) -> bool {
        let changing = self.resized.is_some_and(|t| tick.saturating_sub(t) <= SETTLE);
        let hold = changing && self.held_run < MAX_HOLD && (self.held_run > 0 || self.since_hold >= RUN_BETWEEN);
        if hold {
            self.held_run += 1;
            self.held += 1;
        } else {
            if self.held_run > 0 {
                self.held_run = 0;
                self.since_hold = 0;
            }
            self.since_hold += 1;
        }
        hold
    }

    /// Whether the last period stepped was held.
    pub fn holding(&self) -> bool {
        self.held_run > 0
    }
}

/// `frame` (`fw` × `fh` pixels, rows `fw` apart) scaled to `w` × `h` into
/// `out` (nearest pixel): the last picture shown while held.
pub fn scale_nearest(frame: &[u32], (fw, fh): (usize, usize), out: &mut [u32], (w, h): (usize, usize)) {
    if fw == 0 || fh == 0 || frame.len() < fw * fh {
        let n = (w * h).min(out.len());
        out[..n].fill(0);
        return;
    }
    let xs: Vec<usize> = (0..w).map(|x| x * fw / w).collect();
    for y in 0..h.min(out.len() / w.max(1)) {
        let src = &frame[(y * fh / h) * fw..][..fw];
        let row = &mut out[y * w..(y + 1) * w];
        for (o, &sx) in row.iter_mut().zip(&xs) {
            *o = src[sx];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Held from the period after a change until the size kept still for
    /// SETTLE periods; a change during the hold extends it; at most
    /// MAX_HOLD in a row, then RUN_BETWEEN frames run.
    #[test]
    fn holds_while_the_size_changes_within_bounds() {
        let mut h = ResizeHold::default();
        assert!(!h.step(1));
        h.resized(1);
        let held: Vec<bool> = (2..=10).map(|t| h.step(t)).collect();
        assert_eq!(held, [true, true, true, true, true, true, false, false, false]);
        // Changes every period: held MAX_HOLD in a row, then frames run.
        let mut h = ResizeHold::default();
        let mut run = 0;
        for t in 1..=200u64 {
            h.resized(t - 1);
            if !h.step(t) {
                run += 1;
            } else {
                assert!(run == 0 || run >= RUN_BETWEEN, "held again after {run} frames at {t}");
                run = 0;
            }
        }
        assert_eq!(h.held, MAX_HOLD * 2, "two holds of {MAX_HOLD} in 200 periods");
        // The last change during the hold: held until it settles.
        let mut h = ResizeHold::default();
        h.resized(0);
        assert!(h.step(1) && h.step(2) && h.step(3));
        h.resized(3);
        let held: Vec<bool> = (4..=10).map(|t| h.step(t)).collect();
        assert_eq!(held, [true, true, true, true, true, true, false]);
        assert!(!h.holding());
    }

    #[test]
    fn scaling_nearest() {
        let frame = [1, 2, 3, 4];
        let mut out = vec![0; 16];
        scale_nearest(&frame, (2, 2), &mut out, (4, 4));
        assert_eq!(out, [1, 1, 2, 2, 1, 1, 2, 2, 3, 3, 4, 4, 3, 3, 4, 4]);
    }
}
