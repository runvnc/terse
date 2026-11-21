// license:BSD-3-Clause
// copyright-holders:Hans Andersson, Brad Oliver, Aaron Giles, Bernd Wiebelt, Allard van der Bas
// `BeamInstance` and `instances` are translated from MAME
// src/osd/modules/render/bgfx/vectorrenderer.cpp (`beam_instance`,
// `draw_beams`) and the width model from src/emu/vector.cpp
// (`video_output_update`), commit 6f9a2f023e1a951ba34f289d6a4d214a9578676b,
// for Tersenet. The originals are in third_party/mame-vector-crt/cpp/. The
// rest of the file (Tersenet's lists as MAME's line primitives) is Tersenet's.
//
//! From Tersenet's vector lists to MAME's beam instances.
//!
//! MAME's vector games give the renderer a list of line primitives in the
//! order the game's vector generator drew them: two end points, a colour,
//! an intensity (the primitive's alpha) and a width. A Tersenet list is a
//! display list of strokes and shapes (SPEC §vector.list): it says what is
//! on the screen, not when the beam drew it. So the beam's order is the
//! list's order (the 2D list, then the 3D list's segments, then, in the
//! terminal's driver, the text from the top left), and its timing is
//! MAME's own approximation, which uses only that order: constant
//! deflection speed, each segment's scan time proportional to its length.

use terse_core::Primitive;

/// `beam_instance`: one vector segment for the beam shader (12 floats).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BeamInstance {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub red: f32,
    pub green: f32,
    pub blue: f32,
    pub sigma: f32,
    pub start: f32,
    pub duration: f32,
    pub intensity: f32,
    pub unused: f32,
}

impl BeamInstance {
    pub fn bytes(&self) -> [u8; 48] {
        let f = [self.x0, self.y0, self.x1, self.y1, self.red, self.green, self.blue, self.sigma, self.start, self.duration, self.intensity, self.unused];
        let mut out = [0; 48];
        for (i, v) in f.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        out
    }
}

/// A MAME line primitive (`render_primitive` of type LINE with
/// `PRIMFLAG_VECTOR`), in target pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// `primitive.color.rgb`, 0–1.
    pub rgb: [f32; 3],
    /// `primitive.color.a`: the vector's intensity, 0–1.
    pub intensity: f32,
    /// `primitive.width`, in target pixels.
    pub width: f32,
}

/// `draw_beams`' instance building: the scan timing from accumulated
/// lengths, the sigma from the width.
pub fn instances(lines: &[Line], beam_width: f32) -> Vec<BeamInstance> {
    let length_of = |l: &Line| {
        let (dx, dy) = ((l.x1 - l.x0) as f64, (l.y1 - l.y0) as f64);
        (dx * dx + dy * dy).sqrt().max((l.width as f64).max(1.0))
    };
    let total_length: f64 = lines.iter().map(length_of).sum::<f64>().max(f64::EPSILON);
    let mut elapsed_length = 0.0;
    lines
        .iter()
        .map(|l| {
            let length = length_of(l);
            let i = BeamInstance {
                x0: l.x0,
                y0: l.y0,
                x1: l.x1,
                y1: l.y1,
                red: l.rgb[0],
                green: l.rgb[1],
                blue: l.rgb[2],
                sigma: l.width * beam_width * crate::pipeline::BEAM_SIGMA_SCALE,
                start: (elapsed_length / total_length) as f32,
                duration: (length / total_length) as f32,
                intensity: l.intensity,
                unused: 0.0,
            };
            elapsed_length += length;
            i
        })
        .collect()
}

/// The screen in a target of `size` pixels (Tersenet: the target is the
/// screen, 640 × 480 screen units scaled, as MAME's target is the vector
/// screen's item).
#[derive(Clone, Copy, Debug)]
pub struct Screen {
    pub size: (u32, u32),
}

impl Screen {
    /// Target pixels per screen unit.
    fn scale(&self) -> (f32, f32) {
        (self.size.0 as f32 / 640.0, self.size.1 as f32 / 480.0)
    }

    /// A segment in sixteenths of screen units (SPEC §vector.coordinates)
    /// as a line primitive.
    ///
    /// Width: MAME's vector device gives every vector the width
    /// `beam_width_min / 512` of the screen (the `-beam_width_min` option,
    /// 1.0 by default; `VECTOR_WIDTH_DENOM`), which
    /// `render_target::add_container_primitives` turns into target pixels
    /// with the smaller of the screen's scales (the height, on a 4:3
    /// screen): 1/512 of the height. Tersenet's list states each line's
    /// width in sixteenths of a screen unit, a 480th of the height; it is
    /// kept, so a line of width 16 (one unit) is 512/480 of MAME's default
    /// beam, and wider lines are wider beams.
    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, rgba: [u8; 4], fade: f32) -> Line {
        let (sx, sy) = self.scale();
        let units = |v: f32| v / 16.0;
        Line {
            x0: units(x0) * sx,
            y0: units(y0) * sy,
            x1: units(x1) * sx,
            y1: units(y1) * sy,
            // Tersenet: the colour's bytes over 255, as MAME passes an
            // rgb_t's (sRGB values, which its shader then treats as linear).
            rgb: [rgba[0] as f32 / 255.0, rgba[1] as f32 / 255.0, rgba[2] as f32 / 255.0],
            // Tersenet: opacity as intensity (SPEC §vector.phosphor: "opacity
            // then acts as brightness"), as MAME's intensity is the alpha.
            intensity: rgba[3] as f32 / 255.0 * fade,
            width: units(width.max(1.0)) * sx.min(sy),
        }
    }

    /// Tersenet's primitives (decoded as outlines: nothing is filled in the
    /// phosphor style) as MAME line primitives, in list order. A circle is
    /// a closed polygon (MAME's games draw circles as segments too), with
    /// enough sides that it strays from the true circle by at most a
    /// quarter of a pixel. A line of opacity 0 is not drawn (MAME: a
    /// vector of intensity 0 is a move).
    pub fn lines(&self, prims: &[Primitive], out: &mut Vec<Line>) {
        let (sx, sy) = self.scale();
        for p in prims {
            match *p {
                Primitive::Line { x0, y0, x1, y1, width, rgba } => {
                    out.push(self.line(x0 as f32, y0 as f32, x1 as f32, y1 as f32, width as f32, rgba, 1.0));
                }
                Primitive::Beam { x0, y0, x1, y1, width, rgba, fade, .. } => {
                    // Tersenet: a 3D segment fades from one end to the other;
                    // a MAME vector has one intensity: the mean.
                    let f = (fade[0] as f32 + fade[1] as f32) / 510.0;
                    out.push(self.line(x0 as f32, y0 as f32, x1 as f32, y1 as f32, width as f32, rgba, f));
                }
                Primitive::Circle { x, y, radius, width, rgba } => {
                    self.circle(x as f32, y as f32, radius as f32, width as f32, rgba, (sx, sy), out);
                }
                Primitive::Disc { x, y, radius, rgba } => {
                    self.circle(x as f32, y as f32, radius as f32, 16.0, rgba, (sx, sy), out);
                }
                Primitive::Rect { x0, y0, x1, y1, rgba } => {
                    let (x0, y0, x1, y1) = (x0 as f32, y0 as f32, x1 as f32, y1 as f32);
                    for (a, b, c, d) in [(x0, y0, x1, y0), (x1, y0, x1, y1), (x1, y1, x0, y1), (x0, y1, x0, y0)] {
                        out.push(self.line(a, b, c, d, 16.0, rgba, 1.0));
                    }
                }
                Primitive::Triangle { x0, y0, x1, y1, x2, y2, rgba } => {
                    let p = [(x0 as f32, y0 as f32), (x1 as f32, y1 as f32), (x2 as f32, y2 as f32)];
                    for k in 0..3 {
                        let (a, b) = (p[k], p[(k + 1) % 3]);
                        out.push(self.line(a.0, a.1, b.0, b.1, 16.0, rgba, 1.0));
                    }
                }
            }
        }
        out.retain(|l| l.intensity > 0.0);
    }

    #[allow(clippy::too_many_arguments)]
    fn circle(&self, x: f32, y: f32, r: f32, width: f32, rgba: [u8; 4], scale: (f32, f32), out: &mut Vec<Line>) {
        let r_px = r / 16.0 * scale.0.max(scale.1);
        let n = if r_px <= 0.5 {
            1
        } else {
            let step = (1.0 - 0.25 / r_px).clamp(-1.0, 1.0).acos();
            ((std::f32::consts::PI / step.max(1e-3)).ceil() as usize).clamp(8, 256)
        };
        if n == 1 {
            out.push(self.line(x, y, x, y, width, rgba, 1.0));
            return;
        }
        let at = |k: usize| {
            let a = k as f32 / n as f32 * std::f32::consts::TAU;
            (x + r * a.cos(), y + r * a.sin())
        };
        for k in 0..n {
            let (a, b) = (at(k), at(k + 1));
            out.push(self.line(a.0, a.1, b.0, b.1, width, rgba, 1.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_is_draw_beams() {
        // Two lines of 30 and 10 pixels: the first takes 3/4 of the scan.
        let l = |x0, x1| Line { x0, y0: 0.0, x1, y1: 0.0, rgb: [1.0; 3], intensity: 1.0, width: 2.0 };
        let i = instances(&[l(0.0, 30.0), l(30.0, 40.0)], 0.75);
        assert_eq!((i[0].start, i[0].duration), (0.0, 0.75));
        assert_eq!((i[1].start, i[1].duration), (0.75, 0.25));
        assert!((i[0].sigma - 2.0 * 0.75 * 0.085).abs() < 1e-6);
        // A dot takes max(width, 1 pixel) of scan length.
        let i = instances(&[l(5.0, 5.0), l(0.0, 2.0)], 0.75);
        assert_eq!(i[0].duration, 0.5);
    }

    #[test]
    fn width_follows_the_list() {
        let s = Screen { size: (640, 480) };
        let l = s.line(0.0, 0.0, 160.0, 0.0, 16.0, [255, 255, 255, 255], 1.0);
        assert_eq!((l.x1, l.width), (10.0, 1.0));
        let s = Screen { size: (1280, 960) };
        let l = s.line(0.0, 0.0, 160.0, 0.0, 32.0, [255, 0, 0, 128], 1.0);
        assert_eq!((l.x1, l.width), (20.0, 4.0));
        assert!((l.intensity - 128.0 / 255.0).abs() < 1e-6);
    }
}
