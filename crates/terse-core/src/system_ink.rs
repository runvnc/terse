//! What a system's machines may make a host draw together (SPEC
//! §machines.ink): one budget of square units of the shell's screen, the
//! system ink, shared in increasing handle order, first by the pictures'
//! areas and then by the machines' lists, each counted at the scale it is
//! shown at. The shell's own screen is not counted: it is drawn within its
//! own caps whatever the machines ask.

use crate::devices::Devices;
use crate::machine::Machine;
use crate::shell::System;
use crate::vector3;

/// The system ink (SPEC §machines.ink), in square units of the shell's
/// screen: what one machine shown over the whole screen may count, its
/// picture (640 × 480) and both lists at their caps. Provisional.
pub const SYSTEM_INK: u64 = 640 * 480 + crate::vector::INK_CAP + vector3::INK_CAP as u64;

/// How a machine the shell shows is drawn (SPEC §machines.ink).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Share {
    pub handle: u8,
    /// Shown as a panel in the shell's scene (else flat).
    pub panel: bool,
    /// Its scale, n / 640 (n from 1 to 640).
    pub n: u32,
    /// What its picture counts: its area on the glass.
    pub area: u64,
    /// Its picture did not fit: it is shown as a plain plate, nothing of
    /// its screen drawn.
    pub plate: bool,
    /// The caps its 2D and 3D lists are drawn within, in its own square
    /// screen units (at most [`crate::vector::INK_CAP`] and
    /// [`vector3::INK_CAP`]).
    pub cap2: u64,
    pub cap3: f64,
    /// What its lists counted, at its scale (system ink).
    pub ink: u64,
    /// Its lists drew less than their own caps would have let them.
    pub cut: bool,
}

/// The shares of a system's machines in a frame and the system ink used.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shares {
    pub shares: Vec<Share>,
    pub used: u64,
}

impl Shares {
    /// The share of machine `h`, if it is shown.
    pub fn of(&self, h: u8) -> Option<&Share> {
        self.shares.iter().find(|s| s.handle == h)
    }

    /// Whether any picture is a plate or any list was cut by the system
    /// ink (its cap below the device's own).
    pub fn cut(&self) -> bool {
        self.shares.iter().any(|s| s.plate || s.cut)
    }
}

/// The box of `corners` on the glass seen from the reference eye: its
/// width and height before clipping, and its area clipped to the screen;
/// `None` if a corner is not in front of the near plane.
fn panel_box(corners: &[[f64; 3]; 4]) -> Option<(f64, f64, f64)> {
    if corners.iter().any(|c| c[2] <= vector3::NEAR_Z) {
        return None;
    }
    let g = corners.map(|c| vector3::to_glass(c, (vector3::EYE_X, vector3::EYE_Y)));
    let (x0, x1) = g.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.0), b.max(p.0)));
    let (y0, y1) = g.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.1), b.max(p.1)));
    let w = (x1.min(640.0) - x0.max(0.0)).max(0.0);
    let h = (y1.min(480.0) - y0.max(0.0)).max(0.0);
    Some((x1 - x0, y1 - y0, w * h))
}

impl<D: Devices> System<D> {
    /// The machines' shares of the system ink (SPEC §machines.ink), as they
    /// are shown after the shell's last frame. `ink2` gives the ink of a
    /// machine's 2D list drawn within a cap (SPEC §vector.ink-cap), which
    /// the core of a host that keeps device 3 elsewhere may not hold.
    pub fn shares_with(&self, ink2: &dyn Fn(&Machine<D>, u64) -> u64) -> Shares {
        // The pictures: flat ones, and panels seen from the reference eye
        // with the view's transform where it is.
        let mut shown: Vec<(u8, bool, u32, u64)> = self
            .pictures()
            .into_iter()
            .map(|(h, p)| (h, false, p.w as u32, p.w as u64 * p.h as u64))
            .collect();
        if let Some(scene) = self.scene((0.0, 0.0)) {
            for p in scene.panels.iter().filter(|p| !p.is_surface()) {
                let (n, area) = match panel_box(&p.corners) {
                    None => (640, 640 * 480),
                    Some((w, h, area)) => ((w.max(h * 4.0 / 3.0).ceil() as u32).clamp(1, 640), area.ceil() as u64),
                };
                shown.push((p.handle, true, n, area));
            }
        }
        shown.sort_by_key(|s| s.0);
        let mut out = Shares::default();
        // First the pictures, in increasing handle order.
        for (handle, panel, n, area) in shown {
            let plate = out.used + area > SYSTEM_INK;
            if !plate {
                out.used += area;
            }
            out.shares.push(Share { handle, panel, n, area, plate, cap2: 0, cap3: 0.0, ink: 0, cut: false });
        }
        // Then the lists, in the same order, each at its scale.
        for s in out.shares.iter_mut().filter(|s| !s.plate) {
            let Some(m) = self.child(s.handle).and_then(|c| c.machine.as_ref()) else { continue };
            let n = s.n as u64;
            let room = |used: u64| (SYSTEM_INK - used) * 640 / n;
            s.cap2 = room(out.used).min(crate::vector::INK_CAP);
            let k2 = ink2(m, s.cap2);
            s.cut |= s.cap2 < crate::vector::INK_CAP && k2 < ink2(m, crate::vector::INK_CAP);
            let c2 = (k2 * n).div_ceil(640);
            out.used += c2;
            s.cap3 = (room(out.used) as f64).min(vector3::INK_CAP);
            let k3 = vector3::drawn_ink(&m.list3(), s.cap3);
            s.cut |= s.cap3 < vector3::INK_CAP && k3 < vector3::drawn_ink(&m.list3(), vector3::INK_CAP);
            let c3 = ((k3 * n as f64 / 640.0).ceil() as u64).min(SYSTEM_INK - out.used);
            out.used += c3;
            s.ink = c2 + c3;
        }
        out
    }
}

impl System {
    /// [`System::shares_with`] for a system whose devices are all here.
    pub fn shares(&self) -> Shares {
        self.shares_with(&|m, cap| m.vector().map_or(0, |v| crate::vector::drawn_ink(v.list(), v.outlined(), cap)))
    }
}
